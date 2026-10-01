//! How a request ended, as text and an exit code (design F.5; m3 A.2.3, WP-C1).
//!
//! A request runs through `mklm_client::run_once::run_request`: the helper session, the recovery
//! after a lost helper (at once: the CLI keeps `Frontend::confirm_recovery`'s default, design m2
//! E.7, review C8), and the post-reboot RunOnce rule. Its [`CliFrontend`](super::relay) prints
//! the events and the two lines between sessions (the helper stopped, recovering now); this module
//! prints what is left: the results, the errors and what to do next. It reads the journal again
//! (unelevated) only for that: whether a helper lost during the recovery left something to
//! recover, and what still waits for the user after a recovery (review C7).

use std::fmt;
use std::io::Write;

use anyhow::Result;
use mklm_client::orchestrator::{RequestEnd, RequestReport};
use mklm_client::session::SessionEnd;
use mklm_client::{HelperExit, LaunchError};
use mklm_core::{Journal, JournalEntry, OperationResult, Outcome};

use super::exit_code;
use super::outcome::{error_exit_code, lost_recovery_exit_code, result_exit_code};
use super::render::{entry_line_at, error_text, next_steps, result_text};

/// What to add after a session's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// `recover`: say so when entries still wait for the user (design review C7).
    Recover,
    /// `migrate`: the note about the Settings app once the migration waits for the restart.
    Migrate,
}

/// After a helper was lost with an operation journaled: how to go on (design m2 E.7).
pub const RECOVER_HINT: &str = "Run `mklm-cli recover` to finish or undo the interrupted \
                                operation (`mklm-cli journal` shows where it stopped).";

/// The line that says the helper stopped before it answered (shown by the front end's
/// `Notice::HelperLost`, and here for a helper lost during the recovery).
pub fn helper_stopped_text(exit: Option<HelperExit>, detail: &str) -> String {
    let why = exit.map_or_else(|| detail.to_string(), |exit| exit.to_string());
    format!("error: the helper stopped before it answered: {why}")
}

/// The journal as the report reads it after the request (unelevated).
pub trait JournalAfter {
    fn boot(&self) -> Option<mklm_core::BootId> {
        None
    }
    /// Some entry needs recovery now (`mklm_client::orchestrator::JournalSource`); `Err`: an
    /// English message.
    fn needs_recovery(&mut self) -> Result<bool, String>;
    /// The journal as stored now.
    fn read(&mut self) -> Result<Journal>;
}

/// Writes the report of one request: `out` is standard output, `err` standard error.
pub struct Report<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub journal: &'a mut dyn JournalAfter,
}

impl fmt::Debug for Report<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Report").finish_non_exhaustive()
    }
}

impl Report<'_> {
    /// Prints `text`, ending it with a line break if it has none.
    fn say(&mut self, text: &str) -> Result<()> {
        self.out.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            self.out.write_all(b"\n")?;
        }
        self.out.flush()?;
        Ok(())
    }

    /// Prints `text` (ending with a line break) on standard error, after what standard output
    /// holds. A diagnostic that cannot be written is dropped, as the rest of the report matters
    /// more.
    fn error(&mut self, text: &str) {
        let _ = self.out.flush();
        let _ = self.err.write_all(text.as_bytes());
        if !text.ends_with('\n') {
            let _ = self.err.write_all(b"\n");
        }
        let _ = self.err.flush();
    }

    /// The report of a whole request and its exit code.
    pub fn request(&mut self, report: RequestReport, after: After) -> Result<i32> {
        let RequestReport {
            first,
            lost_needs_recovery,
            recovery,
            recovery_skipped: _,
        } = report;
        let end = match first {
            RequestEnd::NotLaunched(error) => return self.launch_error(&error),
            RequestEnd::Ended(end) => end,
        };
        let SessionEnd::Lost { planned, .. } = end else {
            return self.end(end, after);
        };
        // The front end has said that the helper stopped (`Notice::HelperLost`), and the
        // orchestrator has asked the journal whether to recover.
        let needs_recovery = match lost_needs_recovery {
            Some(Ok(needs)) => needs,
            Some(Err(message)) => {
                self.error(&format!("error: {message}"));
                return Ok(exit_code::FAILURE);
            }
            None => false,
        };
        match recovery {
            // Nothing to recover; or, never in the CLI (it neither cancels nor declines),
            // recovery was skipped (`recovery_skipped`).
            None => {
                if planned || needs_recovery {
                    self.error(RECOVER_HINT);
                }
                Ok(exit_code::FAILURE)
            }
            Some(RequestEnd::NotLaunched(error)) => {
                let code = self.launch_error(&error)?;
                self.error("Run `mklm-cli recover` as soon as possible.");
                Ok(code)
            }
            // Recovery may finish the operation as well as undo it (C.7): the result says which.
            Some(RequestEnd::Ended(SessionEnd::Finished(result))) => {
                self.result(&result, After::Recover)?;
                Ok(lost_recovery_exit_code(&result))
            }
            Some(RequestEnd::Ended(end)) => self.end(end, After::Nothing),
        }
    }

    /// No helper session came up.
    fn launch_error(&mut self, error: &LaunchError) -> Result<i32> {
        match error {
            LaunchError::Declined => {
                self.say(
                    "Cancelled: the administrator permission was declined; nothing was changed.",
                )?;
                Ok(exit_code::CANCELLED)
            }
            LaunchError::Failed { message, .. } => {
                self.error(&format!("error: {message}"));
                Ok(exit_code::FAILURE)
            }
        }
    }

    /// How one session ended (a helper session, the recovery after a lost one, or the
    /// in-process fallback), and the exit code. A lost helper is not followed by a recovery
    /// here: [`Self::request`] shows the one the orchestrator ran.
    pub fn end(&mut self, end: SessionEnd, after: After) -> Result<i32> {
        match end {
            SessionEnd::Finished(result) => {
                self.result(&result, after)?;
                Ok(result_exit_code(&result))
            }
            SessionEnd::Failed(info) => {
                self.error(&error_text(&info));
                Ok(error_exit_code(info.code))
            }
            SessionEnd::Unresponsive => {
                self.error(
                    "error: the helper does not respond. Wait until it ends, or end \
                     mklm-helper.exe in Task Manager, then run `mklm-cli recover`.",
                );
                Ok(exit_code::FAILURE)
            }
            // The CLI's front end never asks to cancel (Ctrl+C ends the process, which closes
            // the pipe); only the GUI leaves a session this way.
            SessionEnd::Abandoned { .. } => {
                self.say("Cancelled.")?;
                Ok(exit_code::CANCELLED)
            }
            SessionEnd::Lost {
                exit_code: helper_code,
                detail,
                planned,
                ..
            } => {
                self.error(&helper_stopped_text(helper_code.map(HelperExit), &detail));
                let needs_recovery = match self.journal.needs_recovery() {
                    Ok(needs) => needs,
                    Err(message) => {
                        self.error(&format!("error: {message}"));
                        return Ok(exit_code::FAILURE);
                    }
                };
                if planned || needs_recovery {
                    self.error(RECOVER_HINT);
                }
                Ok(exit_code::FAILURE)
            }
        }
    }

    fn result(&mut self, result: &OperationResult, after: After) -> Result<()> {
        self.say(&result_text(result))?;
        if let Some(next) = next_steps(result) {
            self.say(&next)?;
        }
        if after == After::Migrate && result.outcome == Outcome::PendingReboot {
            self.say(
                "From now on, choosing \"Use connected keyboard layout\" in Settings is fine \
                 (plan 1.3).",
            )?;
        }
        if after == After::Recover {
            self.recover_note(result)?;
        }
        Ok(())
    }

    /// Design review C7: a recovery that left entries waiting for the user says so.
    fn recover_note(&mut self, result: &OperationResult) -> Result<()> {
        let journal = self.journal.read()?;
        let waiting: Vec<&JournalEntry> = journal
            .open_entries()
            .into_iter()
            .filter(|entry| !entry.state.is_in_flight())
            .collect();
        if result.recovered.is_empty() && waiting.is_empty() {
            self.say("Nothing needed recovery.")?;
        }
        for entry in waiting {
            self.say(&format!(
                "Still waiting for you: {}. `mklm-cli undo` puts it back.",
                entry_line_at(entry, self.journal.boot())
            ))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::LaunchFailure;
    use mklm_core::{
        BootId, ErrorCode, ErrorInfo, FailureReason, LayoutChoice, OpId, OpKind, OpState,
        PendingAction, ProcessIdentity, RecoveredOp, Timestamp,
    };

    use super::*;

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    fn result(outcome: Outcome, failure: Option<FailureReason>) -> OperationResult {
        OperationResult {
            op_id: Some(op()),
            outcome,
            failure,
            pending_action: None,
            conflicts: Vec::new(),
            inv_ps2_violation: None,
            recovered: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// A `Recover` result: one operation found in `from` and left in `to`.
    fn recovered(from: OpState, to: OpState) -> OperationResult {
        let mut recovered = result(Outcome::Recovered, None);
        recovered.op_id = None;
        recovered.recovered.push(RecoveredOp {
            op_id: op(),
            from,
            to,
            decision: "roll-back".into(),
        });
        recovered
    }

    /// An operation that waits for keep or revert.
    fn waiting_entry() -> JournalEntry {
        let id = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".to_string();
        JournalEntry {
            schema_version: 1,
            op_id: OpId::parse("0a1b2c3d-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
            seq: 2,
            kind: OpKind::SetLayout {
                requested: id.clone(),
                instance_ids: vec![id],
                layout: LayoutChoice::Jis,
            },
            state: OpState::AwaitingConfirm,
            boot_id: BootId(1),
            owner: ProcessIdentity {
                pid: 1,
                creation_time: 1,
            },
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            apply: Some(PendingAction::Reconnect),
            countdown: None,
            records: Vec::new(),
            context: Vec::new(),
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: Vec::new(),
        }
    }

    /// The journal after the request, as the fake tells it; counts the reads.
    #[derive(Debug)]
    struct FakeJournal {
        needs_recovery: Result<bool, String>,
        journal: Journal,
        reads: usize,
    }

    impl FakeJournal {
        fn new(needs_recovery: Result<bool, String>) -> Self {
            Self {
                needs_recovery,
                journal: Journal::default(),
                reads: 0,
            }
        }
    }

    impl JournalAfter for FakeJournal {
        fn needs_recovery(&mut self) -> Result<bool, String> {
            self.reads += 1;
            self.needs_recovery.clone()
        }

        fn read(&mut self) -> Result<Journal> {
            self.reads += 1;
            Ok(self.journal.clone())
        }
    }

    /// Exit code, standard output and standard error of a report.
    fn show(
        report: RequestReport,
        after: After,
        journal: &mut FakeJournal,
    ) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = Report {
            out: &mut out,
            err: &mut err,
            journal,
        }
        .request(report, after)
        .unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn ended(end: SessionEnd) -> RequestReport {
        RequestReport {
            first: RequestEnd::Ended(end),
            lost_needs_recovery: None,
            recovery: None,
            recovery_skipped: None,
        }
    }

    fn lost(planned: bool, countdown: bool) -> SessionEnd {
        SessionEnd::Lost {
            exit_code: Some(1),
            detail: "the helper closed the pipe".into(),
            planned,
            countdown,
        }
    }

    /// The first helper was lost; the journal said `needs`; then `recovery`.
    fn lost_then(needs: Result<bool, String>, recovery: Option<RequestEnd>) -> RequestReport {
        RequestReport {
            first: RequestEnd::Ended(lost(true, true)),
            lost_needs_recovery: Some(needs),
            recovery,
            recovery_skipped: None,
        }
    }

    const DECLINED: &str =
        "Cancelled: the administrator permission was declined; nothing was changed.\n";

    #[test]
    fn launch_errors() {
        let report = RequestReport {
            first: RequestEnd::NotLaunched(LaunchError::Declined),
            lost_needs_recovery: None,
            recovery: None,
            recovery_skipped: None,
        };
        let mut journal = FakeJournal::new(Ok(false));
        assert_eq!(
            show(report, After::Nothing, &mut journal),
            (exit_code::CANCELLED, DECLINED.into(), String::new())
        );

        let report = RequestReport {
            first: RequestEnd::NotLaunched(LaunchError::Failed {
                kind: LaunchFailure::HelperMissing,
                message: "mklm-helper.exe was not found next to mklm-cli.exe (gone)".into(),
            }),
            lost_needs_recovery: None,
            recovery: None,
            recovery_skipped: None,
        };
        assert_eq!(
            show(report, After::Nothing, &mut journal),
            (
                exit_code::FAILURE,
                String::new(),
                "error: mklm-helper.exe was not found next to mklm-cli.exe (gone)\n".into()
            )
        );
        assert_eq!(journal.reads, 0);
    }

    #[test]
    fn session_ends() {
        let mut journal = FakeJournal::new(Ok(false));
        let (code, out, err) = show(
            ended(SessionEnd::Finished(result(Outcome::Confirmed, None))),
            After::Nothing,
            &mut journal,
        );
        assert_eq!(code, exit_code::OK);
        assert_eq!(out, "Operation 3f2a9c1e: Done: the change is kept.\n");
        assert_eq!(err, "");

        let (code, out, _) = show(
            ended(SessionEnd::Finished(result(Outcome::PendingReboot, None))),
            After::Migrate,
            &mut journal,
        );
        assert_eq!(code, exit_code::RESTART_REQUIRED);
        assert!(
            out.starts_with("Operation 3f2a9c1e: Written; it takes effect when the PC restarts.\n"),
            "{out}"
        );
        assert!(
            out.contains("Restart the PC with `mklm-cli reboot`"),
            "{out}"
        );
        assert!(
            out.ends_with(
                "From now on, choosing \"Use connected keyboard layout\" in Settings is fine \
                 (plan 1.3).\n"
            ),
            "{out}"
        );

        let busy = ErrorInfo {
            code: ErrorCode::Busy,
            message: "another MKLM process holds the lock".into(),
            op_id: None,
            plan_error: None,
        };
        let (code, out, err) = show(
            ended(SessionEnd::Failed(busy)),
            After::Nothing,
            &mut journal,
        );
        assert_eq!(code, exit_code::BLOCKED);
        assert_eq!(out, "");
        assert_eq!(
            err,
            "error: another MKLM process holds the lock\n  Another MKLM process is writing; try \
             again when it has finished.\n"
        );

        let (code, _, err) = show(
            ended(SessionEnd::Unresponsive),
            After::Nothing,
            &mut journal,
        );
        assert_eq!(code, exit_code::FAILURE);
        assert!(
            err.starts_with("error: the helper does not respond."),
            "{err}"
        );

        let (code, out, _) = show(
            ended(SessionEnd::Abandoned { planned: true }),
            After::Nothing,
            &mut journal,
        );
        assert_eq!((code, out.as_str()), (exit_code::CANCELLED, "Cancelled.\n"));
        assert_eq!(journal.reads, 0);
    }

    #[test]
    fn a_lost_helper_without_a_recovery() {
        // The front end printed the "helper stopped" line; the report adds the way out.
        let mut journal = FakeJournal::new(Ok(true));
        let report = RequestReport {
            first: RequestEnd::Ended(lost(true, false)),
            lost_needs_recovery: Some(Ok(false)),
            recovery: None,
            recovery_skipped: None,
        };
        assert_eq!(
            show(report, After::Nothing, &mut journal),
            (
                exit_code::FAILURE,
                String::new(),
                format!("{RECOVER_HINT}\n")
            )
        );

        // Nothing journaled and nothing to recover: nothing to add.
        let report = RequestReport {
            first: RequestEnd::Ended(lost(false, false)),
            lost_needs_recovery: Some(Ok(false)),
            recovery: None,
            recovery_skipped: None,
        };
        assert_eq!(
            show(report, After::Nothing, &mut journal),
            (exit_code::FAILURE, String::new(), String::new())
        );

        // The journal could not be read: as before, the error and exit code 1.
        let report = lost_then(Err("reading the journal failed: denied".into()), None);
        assert_eq!(
            show(report, After::Nothing, &mut journal),
            (
                exit_code::FAILURE,
                String::new(),
                "error: reading the journal failed: denied\n".into()
            )
        );
        // Everything came from the orchestrator's report.
        assert_eq!(journal.reads, 0);
    }

    #[test]
    fn the_recovery_after_a_lost_helper() {
        // Declined, or not started: say so, and point to `recover`.
        let mut journal = FakeJournal::new(Ok(true));
        let report = lost_then(
            Ok(true),
            Some(RequestEnd::NotLaunched(LaunchError::Declined)),
        );
        assert_eq!(
            show(report, After::Nothing, &mut journal),
            (
                exit_code::CANCELLED,
                DECLINED.into(),
                "Run `mklm-cli recover` as soon as possible.\n".into()
            )
        );

        // Rolled back before the reset (R5): the row says the keyboard never switched, and the
        // exit code tells where the operation ended (put back automatically: 4).
        let report = lost_then(
            Ok(true),
            Some(RequestEnd::Ended(SessionEnd::Finished(recovered(
                OpState::Planned,
                OpState::Reverted,
            )))),
        );
        let (code, out, err) = show(report, After::Nothing, &mut journal);
        assert_eq!(code, exit_code::REVERTED);
        assert_eq!(
            out,
            "Recovery finished.\n  3f2a9c1e: planned (interrupted?) -> reverted (roll-back; \
             stopped before the keyboard reset, so the keyboard never switched)\n"
        );
        assert_eq!(err, "");
        assert_eq!(journal.reads, 1, "the note of `recover` reads the journal");

        // Rolled back during the countdown: as M2 showed it.
        let report = lost_then(
            Ok(true),
            Some(RequestEnd::Ended(SessionEnd::Finished(recovered(
                OpState::AwaitingConfirm,
                OpState::Reverted,
            )))),
        );
        let (_, out, _) = show(report, After::Nothing, &mut journal);
        assert_eq!(
            out,
            "Recovery finished.\n  3f2a9c1e: waiting for keep or revert -> reverted (roll-back)\n"
        );

        // Rolled forward: the change waits for the user, and the note says so.
        journal.journal.entries.push(waiting_entry());
        let report = lost_then(
            Ok(true),
            Some(RequestEnd::Ended(SessionEnd::Finished(recovered(
                OpState::Written,
                OpState::AwaitingConfirm,
            )))),
        );
        let (code, out, _) = show(report, After::Nothing, &mut journal);
        assert_eq!(code, exit_code::AWAITING_CONFIRM);
        assert!(
            out.ends_with(
                "Still waiting for you: 0a1b2c3d  set HID\\VID_3434&PID_D027&MI_00&COL01\\\
                 8&148AD7E3&0&0000 to JIS  (waiting for keep or revert). `mklm-cli undo` puts it \
                 back.\n"
            ),
            "{out}"
        );

        // A recovery that found nothing.
        journal.journal = Journal::default();
        let mut nothing = result(Outcome::Recovered, None);
        nothing.op_id = None;
        let report = lost_then(
            Ok(true),
            Some(RequestEnd::Ended(SessionEnd::Finished(nothing))),
        );
        let (code, out, _) = show(report, After::Nothing, &mut journal);
        assert_eq!(code, exit_code::OK);
        assert_eq!(out, "Recovery finished.\nNothing needed recovery.\n");
    }

    #[test]
    fn a_recovery_helper_lost_as_well() {
        let mut journal = FakeJournal::new(Ok(true));
        let report = lost_then(
            Ok(true),
            Some(RequestEnd::Ended(SessionEnd::Lost {
                exit_code: None,
                detail: "the helper closed the connection".into(),
                planned: false,
                countdown: false,
            })),
        );
        let (code, out, err) = show(report, After::Nothing, &mut journal);
        assert_eq!(code, exit_code::FAILURE);
        assert_eq!(out, "");
        assert_eq!(
            err,
            format!(
                "error: the helper stopped before it answered: the helper closed the \
                 connection\n{RECOVER_HINT}\n"
            )
        );
        assert_eq!(journal.reads, 1);

        let mut journal = FakeJournal::new(Err("reading the boot ID failed: denied".into()));
        let report = lost_then(Ok(true), Some(RequestEnd::Ended(lost(false, false))));
        let (code, _, err) = show(report, After::Nothing, &mut journal);
        assert_eq!(code, exit_code::FAILURE);
        assert!(
            err.ends_with("\nerror: reading the boot ID failed: denied\n"),
            "{err}"
        );
    }

    #[test]
    fn helper_stopped_lines() {
        assert_eq!(
            helper_stopped_text(None, "the helper closed the pipe"),
            "error: the helper stopped before it answered: the helper closed the pipe"
        );
        let text = helper_stopped_text(Some(HelperExit(3)), "ignored");
        assert!(text.contains("build ID"), "{text}");
        assert!(text.ends_with("(exit code 3)"), "{text}");
        assert!(helper_stopped_text(Some(HelperExit(99)), "").contains("exit code 99"));
    }
}
