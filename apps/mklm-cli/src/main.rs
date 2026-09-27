//! `mklm-cli`: command-line interface of Multi Keyboard Layout Manager (MKLM).
//!
//! Every command only reads the system (milestone M1). Output is English for now; the GUI is localized.
//! Results go to standard output; read problems go to standard error as `warning:` lines (and into
//! the `issues` array of the JSON output). Exit code 0 on success, 1 on failure, 2 on usage errors.
//! Redirected output is ASCII-only JSON, or text in the console's code page (as PowerShell and cmd
//! decode it).

#![cfg_attr(not(windows), allow(dead_code))]

mod json;
mod table;
mod text;

use std::borrow::Cow;
use std::io::{self, Write as _};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

/// Oldest Windows build MKLM supports (Windows 11 24H2).
const MIN_BUILD: u32 = 26100;

/// Multi Keyboard Layout Manager: shows which layout (JIS or US) each keyboard types with (read-only)
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

fn main() -> ExitCode {
    // Plan 2.2: before anything else can load a DLL.
    #[cfg(windows)]
    if let Err(error) = mklm_win::restrict_dll_search() {
        eprintln!("warning: could not restrict DLL loading to System32: {error}");
    }
    let cli = Cli::parse();
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
        eprintln!(
            "warning: this is a Remote Desktop session; the remote client's keyboard layout \
             applies to the whole session."
        );
    }
}

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
}
