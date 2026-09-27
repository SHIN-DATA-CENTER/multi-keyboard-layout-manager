//! The post-reboot RunOnce rule (design m2 F.4, review C17; m3 F.2): after every helper session,
//! whatever its end, and when a front end starts, read the journal unelevated and register the
//! post-reboot check for the current user when an entry waits for a restart.
//!
//! Only an unelevated process registers: an elevated one may run as another administrator than
//! the signed-in user, so its `HKCU` would be the wrong account's ([`RunOnce::TellUser`]).
//! The value is written by `mklm_win::session::register_post_reboot` (the only HKCU RunOnce
//! writer, design m2 K).

use std::path::PathBuf;

use mklm_win::{elevation, session};

use crate::gate::{RunOnce, run_once};
use crate::journal::{boot_id, read_journal};

/// Which program the RunOnce entry starts after the restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostRebootCommand {
    /// `"<dir>\mklm-cli.exe" post-reboot` (this executable must be the CLI).
    Cli,
    /// `"<dir>\mklm.exe" --post-reboot`: the GUI next to this executable (plan 3.6).
    Gui,
}

/// The GUI executable's file name.
pub const GUI_EXE: &str = "mklm.exe";

impl PostRebootCommand {
    /// The command line to register: an absolute, quoted executable path and its argument.
    pub fn command_line(self) -> Result<String, RunOnceError> {
        let exe = std::env::current_exe().map_err(|error| {
            RunOnceError::Check(format!("finding this executable failed: {error}"))
        })?;
        Ok(match self {
            PostRebootCommand::Cli => format!("\"{}\" post-reboot", exe.display()),
            PostRebootCommand::Gui => format!("\"{}\" --post-reboot", gui_path(exe).display()),
        })
    }
}

fn gui_path(exe: PathBuf) -> PathBuf {
    exe.parent()
        .map_or_else(|| PathBuf::from(GUI_EXE), |dir| dir.join(GUI_EXE))
}

/// What [`apply_run_once_rule`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOnceOutcome {
    /// Nothing waits for a restart.
    NotNeeded,
    /// Registered this command line.
    Registered(String),
    /// Needed, but this process is elevated (or elevation could not be checked): the caller tells
    /// the user to run the check after the restart.
    TellUser,
}

/// Why the rule could not be applied. The journal keeps the state; the caller warns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOnceError {
    /// The journal or the boot ID could not be read.
    Check(String),
    /// The RunOnce value could not be written.
    Register(String),
}

impl std::fmt::Display for RunOnceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunOnceError::Check(message) | RunOnceError::Register(message) => {
                write!(f, "{message}")
            }
        }
    }
}

/// A request and the RunOnce rule after it (design m2 C17, m3 A.2.3).
#[derive(Debug)]
pub struct RequestOutcome {
    pub report: std::io::Result<crate::orchestrator::RequestReport>,
    pub run_once: Result<RunOnceOutcome, RunOnceError>,
}

/// Runs `request` with the real helper and journal and applies the RunOnce rule for `command`
/// right after it, on the calling thread, whatever the request did. The GUI's session worker
/// calls this, so the rule is done before `SessionEnded` reaches the UI and a quit cannot
/// overtake it; WP-C1 moves the CLI onto it as well.
pub fn run_request(
    launch: crate::launch::LaunchConfig,
    request: mklm_ipc::Request,
    apply: mklm_core::ApplyOptions,
    frontend: &mut dyn crate::session::Frontend,
    command: PostRebootCommand,
) -> RequestOutcome {
    let mut orchestrator = crate::orchestrator::Orchestrator::new(
        crate::launch::HelperLauncher { config: launch },
        crate::journal::LiveJournal,
    );
    let (report, run_once) =
        orchestrator.run_then(request, apply, frontend, || apply_run_once_rule(command));
    RequestOutcome { report, run_once }
}

/// Applies the rule once: reads the journal and, when needed and allowed, registers `command`.
pub fn apply_run_once_rule(command: PostRebootCommand) -> Result<RunOnceOutcome, RunOnceError> {
    let elevated = elevation::is_elevated().unwrap_or(true);
    let journal = read_journal()
        .map_err(|error| RunOnceError::Check(format!("reading the journal failed: {error}")))?;
    let boot = boot_id()
        .map_err(|error| RunOnceError::Check(format!("reading the boot ID failed: {error}")))?;
    match run_once(&journal.journal, boot, elevated) {
        RunOnce::NotNeeded => Ok(RunOnceOutcome::NotNeeded),
        RunOnce::TellUser => Ok(RunOnceOutcome::TellUser),
        RunOnce::Register => {
            let line = command.command_line()?;
            session::register_post_reboot(&line)
                .map_err(|error| RunOnceError::Register(error.to_string()))?;
            Ok(RunOnceOutcome::Registered(line))
        }
    }
}
