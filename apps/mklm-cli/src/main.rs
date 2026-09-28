//! `mklm-cli`: command-line interface of Multi Keyboard Layout Manager (MKLM).
//!
//! `list`, `status`, `global status` (milestone M1) and `journal` only read the system. The write
//! commands of milestone M2 (`set`, `migrate`, `revert`, `restore`, `recover`, `keep`, `reboot`,
//! `post-reboot`) are defined in [`mod@write`]; see docs/design/m2-engine.md, section F.
//! Output is English for now; the GUI is localized.
//! Results go to standard output; read problems go to standard error as `warning:` lines (and into
//! the `issues` array of the JSON output). Read-only commands exit with 0 on success, 1 on failure
//! and 2 on usage errors; write commands use [`write::exit_code`].
//! Redirected output is ASCII-only JSON, or text in the console's code page (as PowerShell and cmd
//! decode it).

#![cfg_attr(not(windows), allow(dead_code))]

mod json;
mod table;
mod text;
mod write;

use std::borrow::Cow;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

/// Oldest Windows build MKLM supports (Windows 11 24H2).
const MIN_BUILD: u32 = 26100;

/// Multi Keyboard Layout Manager: shows and assigns the layout (JIS or US) of each keyboard
#[derive(Debug, Parser)]
#[command(name = "mklm-cli", bin_name = "mklm-cli", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List keyboards with their stored, reported and effective layouts.
    List {
        /// Also list keyboards that are not connected.
        #[arg(long)]
        all: bool,
    },
    /// Show everything read from the system and how MKLM evaluates it.
    Status {
        /// Print one JSON object {snapshot, assessment, issues}.
        #[arg(long)]
        json: bool,
        /// Also include keyboards that are not connected (needed for a complete INV-PS2 check).
        #[arg(long)]
        all: bool,
    },
    /// PC-wide settings: i8042prt\Parameters and input methods.
    Global {
        #[command(subcommand)]
        command: GlobalCommand,
    },
    /// Assign a layout to a keyboard (and to the other collections of the same device).
    Set(write::SetArgs),
    /// Switch from fixed mode to per-keyboard mode (needs one PC restart).
    Migrate(write::MigrateArgs),
    /// Undo an operation (IDs as shown by `mklm-cli journal`).
    Revert(write::RevertArgs),
    /// Undo every change that still waits for you (keep/revert, a restart, or a conflict).
    Undo(write::RecoverArgs),
    /// Decide what the values of an operation in conflict become.
    Resolve(write::ResolveArgs),
    /// Put back the values from before MKLM.
    Restore(write::RestoreArgs),
    /// Finish or undo operations that were interrupted (crash, power loss, closed window).
    Recover(write::RecoverArgs),
    /// Keep an operation that waits for confirmation.
    #[command(visible_alias = "confirm")]
    Keep {
        /// Operation ID, or a unique prefix of at least 8 hex digits.
        op: String,
    },
    /// Restart Windows to apply pending changes (a restart, not a shutdown).
    Reboot {
        #[arg(long)]
        yes: bool,
    },
    /// Started after sign-in by the RunOnce entry: keep or revert changes that needed a restart.
    #[command(hide = true)]
    PostReboot,
    /// Show MKLM's journal of operations (read-only).
    Journal {
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    /// Runs a write command (or `journal`) and returns its exit code; `None` for the read-only
    /// commands handled by [`run`].
    fn run_m2(&self) -> Option<Result<i32>> {
        Some(match self {
            Command::List { .. } | Command::Status { .. } | Command::Global { .. } => return None,
            Command::Set(args) => write::set(args),
            Command::Migrate(args) => write::migrate(args),
            Command::Revert(args) => write::revert(args),
            Command::Undo(args) => write::undo(args),
            Command::Resolve(args) => write::resolve(args),
            Command::Restore(args) => write::restore(args),
            Command::Recover(args) => write::recover(args),
            Command::Keep { op } => write::keep(op),
            Command::Reboot { yes } => write::reboot(*yes),
            Command::PostReboot => write::post_reboot(),
            Command::Journal { json } => {
                write::journal(*json).and_then(|output| print(&output).map(|()| 0))
            }
        })
    }
}

/// Exit code of a write command; codes above 255 (3010) need `process::exit`.
fn exit_with(code: i32) -> ExitCode {
    match u8::try_from(code) {
        Ok(code) => ExitCode::from(code),
        Err(_) => std::process::exit(code),
    }
}

#[derive(Debug, Subcommand)]
enum GlobalCommand {
    /// Show the global values, the mode, the standard layout and input-method warnings.
    Status {
        /// Print one JSON object.
        #[arg(long)]
        json: bool,
    },
}

/// Whether `restrict_dll_search` succeeded at startup. A failure is only a warning here, but the
/// elevated `--in-process` fallback refuses to run without it (design A.7).
static DLL_SEARCH_RESTRICTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn dll_search_restricted() -> bool {
    DLL_SEARCH_RESTRICTED.load(Ordering::SeqCst)
}

fn main() -> ExitCode {
    // Plan 2.2: before anything else can load a DLL.
    #[cfg(windows)]
    match mklm_win::restrict_dll_search() {
        Ok(()) => DLL_SEARCH_RESTRICTED.store(true, Ordering::SeqCst),
        Err(error) => eprintln!("warning: could not restrict DLL loading to System32: {error}"),
    }
    let cli = Cli::parse();
    if let Some(result) = cli.command.run_m2() {
        return match result {
            Ok(code) => exit_with(code),
            Err(error) => {
                eprintln!("error: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    match run(&cli.command).and_then(|output| print(&output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Writes the whole output at once. A closed pipe (e.g. `| Select-Object -First 5`) is not an error.
fn print(output: &str) -> Result<()> {
    let bytes = stdout_bytes(output);
    let mut stdout = io::stdout().lock();
    match stdout.write_all(&bytes).and_then(|()| stdout.flush()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result.context("writing to standard output failed"),
    }
}

/// The console shows any character as is; a pipe or file gets the console's code page, which is
/// what PowerShell and cmd decode native output with.
#[cfg(windows)]
fn stdout_bytes(output: &str) -> Cow<'_, [u8]> {
    use std::io::IsTerminal as _;

    if io::stdout().is_terminal() {
        Cow::Borrowed(output.as_bytes())
    } else {
        Cow::Owned(mklm_win::encode_for_redirected_output(output))
    }
}

#[cfg(not(windows))]
fn stdout_bytes(output: &str) -> Cow<'_, [u8]> {
    Cow::Borrowed(output.as_bytes())
}

/// Runs a command and returns what to print.
#[cfg(windows)]
fn run(command: &Command) -> Result<String> {
    use std::io::IsTerminal as _;

    use mklm_core::assess_with;

    // The console shows any character; a pipe or file goes through a code page (PowerShell 5.1
    // decodes with the OEM one), so redirected JSON is kept ASCII-only.
    let ascii_json = !io::stdout().is_terminal();
    match command {
        Command::List { all } => {
            let report = read_snapshot(*all)?;
            let assessment = assess_with(&report.snapshot, *all);
            Ok(text::list(&report.snapshot, &assessment, *all))
        }
        Command::Status { json, all } => {
            let report = read_snapshot(*all)?;
            let assessment = assess_with(&report.snapshot, *all);
            if *json {
                let issues = json_issues(&report.issues);
                let document = json::StatusDocument {
                    snapshot: &report.snapshot,
                    assessment: &assessment,
                    issues: &issues,
                };
                Ok(json::to_json(&document, ascii_json)?)
            } else {
                Ok(text::status(&report.snapshot, &assessment, *all))
            }
        }
        Command::Global {
            command: GlobalCommand::Status { json },
        } => {
            let mut issues = Vec::new();
            let global = mklm_win::read_global_settings(&mut issues)
                .context("reading the global keyboard settings failed")?;
            let input = mklm_win::read_input_methods(&mut issues);
            match mklm_win::read_os_info(&mut Vec::new()) {
                Ok(os) => warn_os(&os),
                Err(error) => eprintln!("warning: could not check the Windows version: {error}"),
            }
            warn_issues(&issues);
            if *json {
                let issues = json_issues(&issues);
                let document = json::GlobalStatusDocument::new(&global, &input, &issues);
                Ok(json::to_json(&document, ascii_json)?)
            } else {
                Ok(text::global_status(&global, &input))
            }
        }
        _ => unreachable!("M2 commands are dispatched by Command::run_m2"),
    }
}

#[cfg(not(windows))]
fn run(_command: &Command) -> Result<String> {
    anyhow::bail!("mklm-cli runs on Windows only")
}

/// Takes the read-only snapshot and reports its problems on standard error.
#[cfg(windows)]
fn read_snapshot(include_non_present: bool) -> Result<mklm_win::SnapshotReport> {
    let report = mklm_win::snapshot_report(mklm_win::SnapshotOptions {
        include_non_present,
    })
    .context("reading the keyboard state failed")?;
    warn_os(&report.snapshot.os);
    warn_issues(&report.issues);
    Ok(report)
}

fn warn_os(os: &mklm_core::OsInfo) {
    if os.build < MIN_BUILD {
        eprintln!(
            "warning: MKLM supports Windows 11 24H2 (build {MIN_BUILD}) or later; this PC runs \
             build {}, so the results may be wrong.",
            os.build
        );
    }
    if os.remote_session {
        eprintln!("{REMOTE_SESSION_NOTE}");
    }
}

/// The note of a Remote Desktop session (docs/research/rdp-keyboard.md). Whether a new session
/// follows the client's keyboard or the PC's standard layout is not known, so it says neither:
/// only when the table is fixed and when a change that waits for a restart reaches it.
const REMOTE_SESSION_NOTE: &str = "note: this is a Remote Desktop session. Keys typed here come \
    from the Remote Desktop keyboard; its key table is fixed when the session starts and MKLM \
    cannot read which one it is. A change that waits for a restart reaches it only after the \
    restart and a new sign-in. MKLM's per-keyboard layouts apply to the keyboards attached to this \
    PC.";

#[cfg(windows)]
fn warn_issues(issues: &[mklm_win::ReadIssue]) {
    for issue in issues {
        eprintln!("warning: could not read {issue}");
    }
}

#[cfg(windows)]
fn json_issues(issues: &[mklm_win::ReadIssue]) -> Vec<json::Issue> {
    issues
        .iter()
        .map(|issue| json::Issue {
            subject: issue.subject.clone(),
            error: issue.error.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// Accurate whether a new Remote Desktop session follows the client or the PC's standard
    /// layout (not yet known): no claim that it normally follows the client.
    #[test]
    fn the_remote_session_note_says_only_what_is_known() {
        let note = REMOTE_SESSION_NOTE;
        assert!(note.starts_with("note: this is a Remote Desktop session. "));
        assert!(note.contains("its key table is fixed when the session starts"));
        assert!(note.contains("reaches it only after the restart and a new sign-in"));
        for unverified in [
            "follow the client",
            "console",
            "applies to the whole session",
        ] {
            assert!(!note.contains(unverified), "{unverified:?} in {note}");
        }
        assert!(!note.contains("  "), "{note}");
    }

    #[test]
    fn parses_every_command() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.command);
        assert!(matches!(
            parse(&["mklm-cli", "list"]),
            Ok(Command::List { all: false })
        ));
        assert!(matches!(
            parse(&["mklm-cli", "list", "--all"]),
            Ok(Command::List { all: true })
        ));
        assert!(matches!(
            parse(&["mklm-cli", "status", "--json", "--all"]),
            Ok(Command::Status {
                json: true,
                all: true
            })
        ));
        assert!(matches!(
            parse(&["mklm-cli", "global", "status", "--json"]),
            Ok(Command::Global {
                command: GlobalCommand::Status { json: true }
            })
        ));
        assert!(parse(&["mklm-cli"]).is_err());
        assert!(parse(&["mklm-cli", "global"]).is_err());
        assert!(parse(&["mklm-cli", "set"]).is_err());
        assert!(parse(&["mklm-cli", "global", "status", "--all"]).is_err());
    }

    #[test]
    fn parses_m2_commands() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.command);
        assert!(matches!(
            parse(&["mklm-cli", "set", "#2", "--layout", "us"]),
            Ok(Command::Set(write::SetArgs {
                keyboard: write::KeyboardRef::ListRow(2),
                layout: write::LayoutArg::Us,
                apply: write::ApplyArgs {
                    no_reset: false,
                    other_input: false
                },
                ..
            }))
        ));
        assert!(parse(&["mklm-cli", "set", "#2"]).is_err());
        // --yes needs an explicit reset choice (design review S12).
        let id = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
        assert!(parse(&["mklm-cli", "set", id, "--layout", "jis", "--yes"]).is_err());
        assert!(
            parse(&[
                "mklm-cli",
                "set",
                id,
                "--layout",
                "jis",
                "--yes",
                "--other-input"
            ])
            .is_ok()
        );
        assert!(
            parse(&[
                "mklm-cli",
                "set",
                id,
                "--layout",
                "jis",
                "--yes",
                "--no-reset"
            ])
            .is_ok()
        );
        assert!(parse(&["mklm-cli", "restore", "--baseline", "--all", "--yes"]).is_err());
        assert!(matches!(
            parse(&["mklm-cli", "undo", "--other-input"]),
            Ok(Command::Undo(write::RecoverArgs {
                apply: write::ApplyArgs {
                    other_input: true,
                    ..
                },
                in_process: false,
                ..
            }))
        ));
        assert!(matches!(
            parse(&["mklm-cli", "revert", "0f8c2d4e", "--no-reset"]),
            Ok(Command::Revert(write::RevertArgs { .. }))
        ));
        assert!(matches!(
            parse(&[
                "mklm-cli", "resolve", "0f8c2d4e", "--all", "keep-current", "--value", "2=before"
            ]),
            Ok(Command::Resolve(write::ResolveArgs {
                all: Some(write::ChoiceArg::KeepCurrent),
                values,
                ..
            })) if values.len() == 1
        ));
        assert!(parse(&["mklm-cli", "resolve", "0f8c2d4e", "--all", "later"]).is_err());
        assert!(parse(&["mklm-cli", "set", "#2", "--layout", "dvorak"]).is_err());
        assert!(matches!(
            parse(&[
                "mklm-cli", "migrate", "--standard", "jis", "--also", "#2=us", "--also", "#3=standard"
            ]),
            Ok(Command::Migrate(write::MigrateArgs { also, .. })) if also.len() == 2
        ));
        assert!(matches!(
            parse(&["mklm-cli", "restore", "--baseline", "--all"]),
            Ok(Command::Restore(write::RestoreArgs { all: true, .. }))
        ));
        // Exactly one of --all and a keyboard; --baseline is required.
        assert!(parse(&["mklm-cli", "restore", "--baseline"]).is_err());
        assert!(parse(&["mklm-cli", "restore", "--baseline", "--all", "#1"]).is_err());
        assert!(parse(&["mklm-cli", "restore", "--all"]).is_err());
        assert!(matches!(
            parse(&["mklm-cli", "confirm", "0f8c2d4e"]),
            Ok(Command::Keep { .. })
        ));
        assert!(matches!(
            parse(&["mklm-cli", "post-reboot"]),
            Ok(Command::PostReboot)
        ));
        assert!(matches!(
            parse(&["mklm-cli", "journal", "--json"]),
            Ok(Command::Journal { json: true })
        ));
    }

    #[test]
    fn parses_dry_runs() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.command);
        assert!(matches!(
            parse(&["mklm-cli", "set", "#2", "--layout", "jis", "--dry-run"]),
            Ok(Command::Set(write::SetArgs { dry_run: true, .. }))
        ));
        assert!(matches!(
            parse(&["mklm-cli", "migrate", "--dry-run"]),
            Ok(Command::Migrate(write::MigrateArgs { dry_run: true, .. }))
        ));
        assert!(matches!(
            parse(&["mklm-cli", "restore", "--baseline", "--all", "--dry-run"]),
            Ok(Command::Restore(write::RestoreArgs { dry_run: true, .. }))
        ));
        assert!(matches!(
            parse(&["mklm-cli", "undo", "--dry-run"]),
            Ok(Command::Undo(write::RecoverArgs { dry_run: true, .. }))
        ));
        assert!(matches!(
            parse(&["mklm-cli", "recover", "--dry-run"]),
            Ok(Command::Recover(write::RecoverArgs { dry_run: true, .. }))
        ));
        // Not on the commands that have nothing to plan.
        assert!(parse(&["mklm-cli", "revert", "0f8c2d4e", "--dry-run"]).is_err());
        assert!(parse(&["mklm-cli", "keep", "0f8c2d4e", "--dry-run"]).is_err());
    }

    /// `--yes` with `#n` is a usage error (exit code 2), refused before anything is read or
    /// launched (design F.1, review S12).
    #[test]
    fn yes_with_list_rows_is_a_usage_error() {
        let command = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.command).unwrap();
        for args in [
            &[
                "mklm-cli",
                "set",
                "#2",
                "--layout",
                "jis",
                "--yes",
                "--other-input",
            ][..],
            &[
                "mklm-cli",
                "set",
                "#2",
                "--layout",
                "jis",
                "--yes",
                "--no-reset",
                "--dry-run",
            ][..],
            &["mklm-cli", "migrate", "--also", "#2=us", "--yes"][..],
            &[
                "mklm-cli",
                "restore",
                "--baseline",
                "#2",
                "--yes",
                "--no-reset",
            ][..],
        ] {
            let result = command(args).run_m2().expect("a write command");
            assert_eq!(result.ok(), Some(write::exit_code::USAGE), "{args:?}");
        }
    }
}
