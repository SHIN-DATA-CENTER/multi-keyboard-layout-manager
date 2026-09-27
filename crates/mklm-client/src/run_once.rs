//! The post-reboot RunOnce rule (design m2 F.4, review C17; m3 F.2): after every helper session,
//! whatever its end, and when a front end starts, read the journal unelevated and register the
//! post-reboot check for the current user when an entry waits for a restart.
//!
//! Only an unelevated process registers: an elevated one may run as another administrator than
//! the signed-in user, so its `HKCU` would be the wrong account's ([`RunOnce::TellUser`]).
//! The value is written by `mklm_win::session::register_post_reboot` (the only HKCU RunOnce
//! writer, design m2 K).
//!
//! Which program the entry starts: the GUI always registers itself; the CLI registers the GUI
//! next to it when that GUI is of the same build, and itself otherwise
//! ([`PostRebootCommand::preferred`], design m3 F.2, K.7).

use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

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

/// `FILE_ATTRIBUTE_REPARSE_POINT` (winnt.h).
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

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

    /// The command the CLI registers (design m3 F.2, K.7; m2 F.4 "the GUI when it is installed"):
    /// [`Self::Gui`] when a `mklm.exe` next to this executable carries `build_id` (this
    /// executable's `env!("MKLM_BUILD_ID")`, apps/build_id.rs), [`Self::Cli`] otherwise. A GUI of
    /// another build, or one whose build ID cannot be read, is never started after the restart:
    /// it may not agree with this build on the journal and the helper. Reads the GUI's VERSIONINFO
    /// only (no process is started); the GUI itself always registers [`Self::Gui`].
    pub fn preferred(build_id: &str) -> Self {
        let gui_build_id = std::env::current_exe()
            .ok()
            .and_then(|exe| adjacent_gui_build_id(&exe));
        Self::for_gui_build(gui_build_id.as_deref(), build_id)
    }

    /// [`Self::preferred`] once the adjacent GUI's build ID is known (`None`: no GUI, or none
    /// whose build ID could be read).
    fn for_gui_build(gui_build_id: Option<&str>, build_id: &str) -> Self {
        match gui_build_id {
            Some(found) if !build_id.is_empty() && found == build_id => Self::Gui,
            _ => Self::Cli,
        }
    }
}

fn gui_path(exe: PathBuf) -> PathBuf {
    exe.parent()
        .map_or_else(|| PathBuf::from(GUI_EXE), |dir| dir.join(GUI_EXE))
}

/// The build ID in the VERSIONINFO of the `mklm.exe` next to `exe` ([`gui_path`], the path
/// [`PostRebootCommand::command_line`] registers), when that is a regular file and not a reparse
/// point (as `mklm_win::elevation::helper_path` checks the helper).
fn adjacent_gui_build_id(exe: &Path) -> Option<String> {
    let gui = gui_path(exe.to_path_buf());
    let metadata = std::fs::symlink_metadata(&gui).ok()?;
    let plain = gui.is_absolute()
        && metadata.is_file()
        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0;
    if !plain {
        return None;
    }
    elevation::file_build_id(&gui).ok()
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
/// overtake it; the CLI runs every helper request through it too (WP-C1), and shows the report
/// and the rule's outcome afterwards.
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const BUILD: &str = "0.1.0+0123456789abcdef0123456789abcdef";

    #[test]
    fn the_cli_registers_the_gui_of_its_own_build_only() {
        assert_eq!(
            PostRebootCommand::for_gui_build(Some(BUILD), BUILD),
            PostRebootCommand::Gui
        );
        assert_eq!(
            PostRebootCommand::for_gui_build(Some("0.1.0+ffffffffffffffffffffffffffffffff"), BUILD),
            PostRebootCommand::Cli
        );
        assert_eq!(
            PostRebootCommand::for_gui_build(None, BUILD),
            PostRebootCommand::Cli
        );
        assert_eq!(
            PostRebootCommand::for_gui_build(Some(""), ""),
            PostRebootCommand::Cli
        );
    }

    /// A scratch directory under the temporary directory, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let dir = std::env::temp_dir()
                .join(format!("mklm-client-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn no_build_id_without_a_readable_gui_next_to_the_executable() {
        // No mklm.exe at all.
        let empty = Scratch::new("empty");
        assert_eq!(adjacent_gui_build_id(&empty.0.join("mklm-cli.exe")), None);

        // A directory by that name is not a program.
        let directory = Scratch::new("directory");
        fs::create_dir(directory.0.join(GUI_EXE)).unwrap();
        assert_eq!(
            adjacent_gui_build_id(&directory.0.join("mklm-cli.exe")),
            None
        );

        // A file without VERSIONINFO has no build ID.
        let junk = Scratch::new("junk");
        fs::write(junk.0.join(GUI_EXE), b"not a program").unwrap();
        assert_eq!(adjacent_gui_build_id(&junk.0.join("mklm-cli.exe")), None);
        assert_eq!(gui_path(junk.0.join("mklm-cli.exe")), junk.0.join(GUI_EXE));
    }

    #[test]
    fn the_test_executable_has_no_gui_next_to_it() {
        // Test executables live in target\<profile>\deps, where Cargo keeps `mklm-<hash>.exe`
        // but no `mklm.exe`.
        assert_eq!(PostRebootCommand::preferred(BUILD), PostRebootCommand::Cli);
    }
}
