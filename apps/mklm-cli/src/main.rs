//! `mklm-cli`: command-line interface of Multi Keyboard Layout Manager (MKLM).
//!
//! `list`, `status`, `global status` (milestone M1) and `journal` only read the system. The write
//! commands of milestone M2 (`set`, `migrate`, `revert`, `restore`, `recover`, `keep`, `reboot`,
//! `post-reboot`) are defined in [`mod@write`]; see docs/design/m2-engine.md, section F.
//! `update --check` and `update --status` (M5b) are in [`mod@update`]; see
//! docs/design/m5b-updater.md, D.14.
//! Output is English for now; the GUI is localized.
//! Results go to standard output; read problems go to standard error as `warning:` lines (and into
//! the `issues` array of the JSON output). Read-only commands exit with 0 on success, 1 on failure
//! and 2 on usage errors; write commands use [`write::exit_code`]; `update --check` has its own
//! codes (0 / 20 / 21 / 22 / 6 / 1 / 2). While an MKLM update runs, the write commands and
//! `update --check` end at once with 6 ([`update::RUNNING_MESSAGE`]); the read-only commands are
//! never held up.
//! Redirected output is ASCII-only JSON, or text in the console's code page (as PowerShell and cmd
//! decode it).

#![cfg_attr(not(windows), allow(dead_code))]

mod json;
mod table;
mod text;
mod update;
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
    /// Change the PC's standard layout in per-keyboard mode (one PC restart).
    Standard(write::StandardArgs),
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
    /// Check for a new version of MKLM (--check), or show the state of updates (--status).
    /// Installing is done by MKLM itself, or by running the installer.
    Update(update::UpdateArgs),
}

impl Command {
    /// True for the commands an MKLM update holds up (design m5b D.14): the write commands and
    /// `update --check`. The read-only ones (`list`, `status`, `global status`, `journal`,
    /// `update --status`) end within a second and are never held up (FIX-VERIFICATION-14).
    fn waits_for_updates(&self) -> bool {
        match self {
            Command::Set(_)
            | Command::Migrate(_)
            | Command::Standard(_)
            | Command::Revert(_)
            | Command::Undo(_)
            | Command::Resolve(_)
            | Command::Restore(_)
            | Command::Recover(_)
            | Command::Keep { .. }
            | Command::Reboot { .. }
            | Command::PostReboot => true,
            Command::Update(args) => args.check,
            Command::List { .. }
            | Command::Status { .. }
            | Command::Global { .. }
            | Command::Journal { .. } => false,
        }
    }

    /// Runs a write command (or `journal`, `update`) and returns its exit code; `None` for the
    /// read-only commands handled by [`run`].
    fn run_m2(&self) -> Option<Result<i32>> {
        Some(match self {
            Command::List { .. } | Command::Status { .. } | Command::Global { .. } => return None,
            #[cfg(windows)]
            Command::Update(args) => update::run(args),
            #[cfg(not(windows))]
            Command::Update(_) => Err(anyhow::anyhow!("mklm-cli runs on Windows only")),
            Command::Set(args) => write::set(args),
            Command::Migrate(args) => write::migrate(args),
            Command::Standard(args) => write::standard(args),
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

/// Whether `restrict_dll_search` and `pin_system_environment` succeeded at startup. A failure is
/// only a warning here, but the elevated `--in-process` fallback refuses to run without them
/// (design A.7, m5b D.9.4).
static DLL_SEARCH_RESTRICTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn dll_search_restricted() -> bool {
    DLL_SEARCH_RESTRICTED.load(Ordering::SeqCst)
}

fn main() -> ExitCode {
    // Plan 2.2: before anything else can load a DLL. Then SystemDrive, SystemRoot and windir from
    // the system, before any known-folder lookup (the first answer is cached; design m5b D.9.4):
    // an elevated console's environment carries what the unelevated user set in HKCU\Environment,
    // and the `--in-process` engine finds %ProgramData% through %SystemDrive%.
    #[cfg(windows)]
    match mklm_win::restrict_dll_search()
        .and_then(|()| mklm_win::elevation::pin_system_environment())
    {
        Ok(()) => DLL_SEARCH_RESTRICTED.store(true, Ordering::SeqCst),
        Err(error) => eprintln!(
            "warning: could not restrict DLL loading to System32 and take the system folders \
             from Windows: {error}"
        ),
    }
    let cli = Cli::parse();
    // An MKLM update past `ready` waits for this program to end (design m5b D.14).
    #[cfg(windows)]
    if cli.command.waits_for_updates() && update::blocked_by_update() {
        eprintln!("{}", update::RUNNING_MESSAGE);
        return exit_with(update::UPDATE_RUNNING);
    }
    // Every helper session of this process first reports the user's newest verified update
    // information when the machine's record is behind (design m5b C.4).
    #[cfg(windows)]
    let _ = mklm_client::session::set_trust_reporter(Box::new(
        mklm_client::update::trust_report::CurrentUser::default(),
    ));
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
            let (journal, error) = read_journal_status();
            Ok(text::list(&report.snapshot, &assessment, *all)
                + &json::journal_note(&journal, error.as_deref()))
        }
        Command::Status { json, all } => {
            let report = read_snapshot(*all)?;
            let assessment = assess_with(&report.snapshot, *all);
            let (journal, journal_error) = read_journal_status();
            if *json {
                let issues = json_issues(&report.issues);
                let document = json::StatusDocument {
                    snapshot: &report.snapshot,
                    assessment: &assessment,
                    issues: &issues,
                    journal,
                    journal_error,
                };
                Ok(json::to_json(&document, ascii_json)?)
            } else {
                Ok(text::status(&report.snapshot, &assessment, *all)
                    + &json::journal_note(&journal, journal_error.as_deref()))
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

#[cfg(windows)]
fn read_journal_status() -> (Vec<json::JournalStatus>, Option<String>) {
    match mklm_client::journal::read_journal() {
        Ok(read) => {
            let boot = mklm_client::journal::boot_id();
            let rows = json::journal_status(&read.journal, boot.as_ref().ok().copied());
            let error = if !read.journal.unreadable.is_empty() {
                Some(format!(
                    "{} unreadable entries",
                    read.journal.unreadable.len()
                ))
            } else {
                boot.err().map(|error| error.to_string())
            };
            (rows, error)
        }
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
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
/// how or when the session's key table is selected remains unverified.
const REMOTE_SESSION_NOTE: &str = "note: this is a Remote Desktop session. Keys typed here come \
    from the PC you connect from, through one Remote Desktop keyboard and one session key table. \
    MKLM's per-keyboard assignments affect keyboards attached to the PC you connect to; they do \
    not change the session's key table. How the host standard and client report determine that \
    table, and when a standard change reaches it, are not yet verified. Check Shift+2 in the session.";

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
        assert!(note.contains("one session key table"));
        assert!(note.contains("are not yet verified"));
        assert!(!note.contains("fixed when the session starts"));
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
            parse(&["mklm-cli", "standard", "us", "--dry-run"]),
            Ok(Command::Standard(write::StandardArgs { dry_run: true, .. }))
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

    #[test]
    fn standard_accepts_repeated_followers_and_requires_a_reset_choice_with_yes() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.command);
        let id = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
        assert!(matches!(
            parse(&["mklm-cli", "standard", "jis", "--follow", id, "--follow", "#2"]),
            Ok(Command::Standard(write::StandardArgs {
                standard: write::StandardArg::Jis, follow, ..
            })) if follow == vec![write::KeyboardRef::InstanceId(id.into()), write::KeyboardRef::ListRow(2)]
        ));
        for choice in ["--other-input", "--no-reset"] {
            assert!(parse(&["mklm-cli", "standard", "us", "--yes", choice]).is_ok());
        }
        for bad in [
            &["mklm-cli", "standard"][..],
            &["mklm-cli", "standard", "standard"][..],
            &["mklm-cli", "standard", "us", "--yes"][..],
            &["mklm-cli", "standard", "us", "--follow"][..],
            &["mklm-cli", "standard", "us", "--other-input", "--no-reset"][..],
        ] {
            assert_eq!(parse(bad).unwrap_err().exit_code(), 2, "{bad:?}");
        }
    }

    /// `update` takes exactly one of `--check` and `--status` (a usage error, exit code 2,
    /// otherwise), and only `--check` and the write commands wait for a running update (design
    /// m5b D.14; FIX-VERIFICATION-14).
    #[test]
    fn update_commands() {
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|cli| cli.command);
        let check = parse(&["mklm-cli", "update", "--check", "--json"]).unwrap();
        assert!(matches!(
            &check,
            Command::Update(update::UpdateArgs {
                check: true,
                status: false,
                json: true,
                ..
            })
        ));
        assert!(check.waits_for_updates());
        let status = parse(&["mklm-cli", "update", "--status"]).unwrap();
        assert!(!status.waits_for_updates());
        for bad in [
            &["mklm-cli", "update"][..],
            &["mklm-cli", "update", "--check", "--status"][..],
            &["mklm-cli", "update", "--install"][..],
            &["mklm-cli", "update", "--json"][..],
        ] {
            let error = parse(bad).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{bad:?}");
        }
        // The development endpoint does not exist in this build.
        #[cfg(not(all(debug_assertions, mklm_update_dev)))]
        assert!(
            parse(&[
                "mklm-cli",
                "update",
                "--check",
                "--update-endpoint",
                "http://127.0.0.1:8080"
            ])
            .is_err()
        );
        for (args, waits) in [
            (&["mklm-cli", "list"][..], false),
            (&["mklm-cli", "status"][..], false),
            (&["mklm-cli", "global", "status"][..], false),
            (&["mklm-cli", "journal"][..], false),
            (&["mklm-cli", "keep", "0f8c2d4e"][..], true),
            (&["mklm-cli", "reboot"][..], true),
            (&["mklm-cli", "post-reboot"][..], true),
            (&["mklm-cli", "recover"][..], true),
            (&["mklm-cli", "undo"][..], true),
            (&["mklm-cli", "revert", "0f8c2d4e"][..], true),
            (&["mklm-cli", "restore", "--baseline", "--all"][..], true),
            (&["mklm-cli", "set", "#1", "--layout", "jis"][..], true),
            (&["mklm-cli", "migrate"][..], true),
            (&["mklm-cli", "standard", "jis"][..], true),
            (&["mklm-cli", "standard", "jis", "--dry-run"][..], true),
            (
                &["mklm-cli", "resolve", "0f8c2d4e", "--all", "keep-current"][..],
                true,
            ),
        ] {
            assert_eq!(parse(args).unwrap().waits_for_updates(), waits, "{args:?}");
        }
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
                "standard",
                "jis",
                "--follow",
                "#2",
                "--yes",
                "--no-reset",
            ][..],
            &[
                "mklm-cli",
                "standard",
                "us",
                "--follow",
                "#2",
                "--yes",
                "--other-input",
                "--dry-run",
            ][..],
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
