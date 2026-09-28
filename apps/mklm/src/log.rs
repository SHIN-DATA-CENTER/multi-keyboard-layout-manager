//! The GUI's log (design m3 F.8): `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\logs\mklm.log`, in
//! English, one line per event, turned over to `mklm.log.1` at 1 MB (one old generation). A small
//! writer of its own: M3 adds no logging crate (design m3 K.10).
//!
//! What is logged: start and end, ignored arguments, read problems, the single instance, helper
//! sessions (the request's kind, how it ended, the English diagnostics), the end of the Windows
//! session, watcher, save and autostart errors.
//!
//! What is never logged (plan 2.2): key characters, scan codes, key-test contents. Callers pass
//! English text composed for the log — never the `Debug` form of an `AppMsg` (a key test carries
//! the typed text) or anything from `input_capture`. A message is kept on one line
//! ([`format_line`]), so that no text can forge another entry.
//!
//! Every line also goes to standard error: the debug build is a console program (design m3 F.7);
//! the release build has no console and the text is dropped. Logging never fails the caller: a
//! line that cannot be written is skipped.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// The log's file name inside the log folder.
pub const LOG_FILE: &str = "mklm.log";

/// The previous generation (replaced at each turn-over).
pub const OLD_LOG_FILE: &str = "mklm.log.1";

/// Size at which the log is turned over.
pub const LOG_LIMIT: u64 = 1024 * 1024;

/// How serious a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// One log line, `<time> [<level>] <message>` and a line break. Control characters of the message
/// (line breaks included) become spaces: one event is always one line.
pub fn format_line(time: &str, level: Level, message: &str) -> String {
    let mut line = format!("{time} [{}] ", level.label());
    line.extend(
        message
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c }),
    );
    line.push_str("\r\n");
    line
}

/// The log file in one folder, turned over at a size limit.
#[derive(Debug)]
pub struct LogFile {
    dir: PathBuf,
    limit: u64,
    /// Opened (and the folder created) on the first line.
    file: Option<File>,
}

impl LogFile {
    pub fn new(dir: PathBuf, limit: u64) -> Self {
        Self {
            dir,
            limit,
            file: None,
        }
    }

    /// Appends a formatted `line`. When the file would pass the limit, it becomes
    /// [`OLD_LOG_FILE`] first (replacing the older one) and a new file starts. The size is read
    /// from the file each time, so a second MKLM process writing to it (a second start that
    /// activates the first) is counted too.
    pub fn append(&mut self, line: &str) -> io::Result<()> {
        let path = self.dir.join(LOG_FILE);
        let file = match self.file.take() {
            Some(file) => file,
            None => {
                fs::create_dir_all(&self.dir)?;
                open_append(&path)?
            }
        };
        let size = file.metadata()?.len();
        let mut file = if size > 0 && size.saturating_add(line.len() as u64) > self.limit {
            // Closed before the rename. When the old log cannot be moved (another program holds
            // it), the log starts over, so that it stays bounded.
            drop(file);
            if fs::rename(&path, self.dir.join(OLD_LOG_FILE)).is_err() {
                File::create(&path)?;
            }
            open_append(&path)?
        } else {
            file
        };
        let written = file.write_all(line.as_bytes());
        self.file = Some(file);
        written
    }
}

fn open_append(path: &std::path::Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// The process's log; `None` until [`init`] (tests never call it: their lines go to standard
/// error only).
static LOG: Mutex<Option<LogFile>> = Mutex::new(None);

/// Starts writing to `dir` (the log folder), or to standard error only when there is none.
pub fn init(dir: Option<PathBuf>) {
    *LOG.lock().unwrap_or_else(PoisonError::into_inner) =
        dir.map(|dir| LogFile::new(dir, LOG_LIMIT));
}

pub fn info(message: impl AsRef<str>) {
    write(Level::Info, message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write(Level::Warn, message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    write(Level::Error, message.as_ref());
}

/// Writes one line to standard error and to the log file.
pub fn write(level: Level, message: &str) {
    let line = format_line(&now_text(), level, message);
    let _ = io::stderr().lock().write_all(line.as_bytes());
    if let Some(file) = LOG.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = file.append(&line);
    }
}

/// Now, in local time with the offset from UTC (as the history shows times, design m3 G.1), or
/// in UTC when the local time cannot be computed.
fn now_text() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    local_text(ms).unwrap_or_else(|| utc_text(ms))
}

#[cfg(windows)]
fn local_text(ms: u64) -> Option<String> {
    mklm_win::time::local_time(mklm_core::Timestamp(ms))
        .ok()
        .map(|local| local.to_iso_text())
}

#[cfg(not(windows))]
fn local_text(_ms: u64) -> Option<String> {
    None
}

/// The kind of a request, for the log: the variant's name (`SetLayout`, `Recover` …). Never its
/// fields.
pub fn request_kind(request: &mklm_ipc::Request) -> &'static str {
    use mklm_ipc::Request;
    match request {
        Request::SetLayout(_) => "SetLayout",
        Request::Migrate(_) => "Migrate",
        Request::Revert { .. } => "Revert",
        Request::Confirm { .. } => "Confirm",
        Request::RestoreBaseline(_) => "RestoreBaseline",
        Request::Recover { .. } => "Recover",
        Request::Undo { .. } => "Undo",
        Request::ResolveConflict(_) => "ResolveConflict",
        Request::CleanupValues { .. } => "CleanupValues",
        Request::SetMachineSettings { .. } => "SetMachineSettings",
    }
}

/// A session notice, for the log.
pub fn notice_text(notice: &mklm_client::session::Notice) -> String {
    use mklm_client::session::Notice;
    match notice {
        Notice::Starting(kind) => format!("launching the helper ({kind:?})"),
        Notice::Connected(kind) => format!("the helper is connected ({kind:?})"),
        Notice::HelperLost { exit, detail } => match exit {
            Some(exit) => format!("the helper stopped ({exit}): {detail}"),
            None => format!("the helper stopped: {detail}"),
        },
        Notice::RecoveringAfterLoss { countdown } => {
            format!("recovering after the lost helper (countdown: {countdown})")
        }
    }
}

/// How a session ended, for the log: the class of the result (`OutcomeClass`), the English
/// diagnostics, the recovery after a lost helper and the RunOnce rule.
pub fn session_end_text(outcome: &crate::state::SessionOutcome) -> String {
    let report = &outcome.report;
    let mut text = request_end_text(&report.first);
    if let Some(needed) = &report.lost_needs_recovery {
        match needed {
            Ok(needed) => text.push_str(&format!("; recovery needed: {needed}")),
            Err(error) => text.push_str(&format!("; recovery needed: unknown ({error})")),
        }
    }
    if let Some(recovery) = &report.recovery {
        text.push_str(&format!("; recovery: {}", request_end_text(recovery)));
    }
    if let Some(skipped) = &report.recovery_skipped {
        text.push_str(&format!("; recovery not started: {skipped:?}"));
    }
    match &outcome.run_once {
        Ok(done) => text.push_str(&format!("; post-reboot RunOnce: {done:?}")),
        Err(error) => text.push_str(&format!("; post-reboot RunOnce failed: {error}")),
    }
    text
}

fn request_end_text(end: &mklm_client::orchestrator::RequestEnd) -> String {
    use mklm_client::orchestrator::RequestEnd;
    use mklm_client::outcome::{classify_error, classify_result};
    use mklm_client::session::SessionEnd;
    match end {
        RequestEnd::NotLaunched(error) => format!("not launched: {error}"),
        RequestEnd::Ended(SessionEnd::Finished(result)) => {
            let mut text = format!(
                "{:?} (outcome {:?}",
                classify_result(result),
                result.outcome
            );
            if let Some(op) = &result.op_id {
                text.push_str(&format!(", op {op}"));
            }
            if let Some(failure) = &result.failure {
                text.push_str(&format!(", reason {failure:?}"));
            }
            if let Some(action) = &result.pending_action {
                text.push_str(&format!(", pending {action:?}"));
            }
            text.push(')');
            for warning in &result.warnings {
                text.push_str(&format!("; warning: {warning}"));
            }
            text
        }
        RequestEnd::Ended(SessionEnd::Failed(info)) => format!(
            "{:?} ({:?}): {}",
            classify_error(info.code),
            info.code,
            info.message
        ),
        RequestEnd::Ended(SessionEnd::Lost {
            exit_code,
            detail,
            planned,
            countdown,
        }) => format!(
            "the helper was lost (exit code {exit_code:?}, journaled {planned}, countdown {countdown}): {detail}"
        ),
        RequestEnd::Ended(SessionEnd::Unresponsive) => "the helper stopped answering".to_string(),
        RequestEnd::Ended(SessionEnd::Abandoned { planned }) => {
            format!("left by MKLM (journaled {planned})")
        }
    }
}

/// `YYYY-MM-DD HH:MM:SS UTC` for milliseconds since the Unix epoch.
pub fn utc_text(ms: u64) -> String {
    let seconds = ms / 1000;
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder under the temporary directory, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            Self(std::env::temp_dir().join(format!(
                "mklm-log-test-{tag}-{}-{nanos}",
                std::process::id()
            )))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn one_event_is_one_line() {
        assert_eq!(
            format_line(
                "2026-09-28 04:48:20 +09:00",
                Level::Warn,
                "a\nforged [info] b\r"
            ),
            "2026-09-28 04:48:20 +09:00 [warn] a forged [info] b \r\n"
        );
        assert_eq!(
            format_line("t", Level::Info, "session 1 started: set-layout"),
            "t [info] session 1 started: set-layout\r\n"
        );
        assert!(format_line("t", Level::Error, "x").starts_with("t [error] "));
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_text(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_text(951_782_400_000), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc_text(1_700_000_000_999), "2023-11-14 22:13:20 UTC");
        assert_eq!(utc_text(4_107_542_400_000), "2100-03-01 00:00:00 UTC");
    }

    #[test]
    fn the_log_turns_over_at_its_limit() {
        let scratch = Scratch::new("turn-over");
        let mut log = LogFile::new(scratch.0.clone(), 64);
        let line = format_line("t", Level::Info, "0123456789012345678901234"); // 36 bytes
        log.append(&line).unwrap();
        // A second line would pass 64 bytes: the first one moves to mklm.log.1.
        log.append(&line).unwrap();
        assert_eq!(
            fs::read_to_string(scratch.0.join(OLD_LOG_FILE)).unwrap(),
            line
        );
        assert_eq!(fs::read_to_string(scratch.0.join(LOG_FILE)).unwrap(), line);
        let short = format_line("t", Level::Info, "x");
        log.append(&short).unwrap();
        assert_eq!(
            fs::read_to_string(scratch.0.join(LOG_FILE)).unwrap(),
            format!("{line}{short}")
        );
        // One old generation only: the next turn-over replaces it.
        log.append(&line).unwrap();
        assert_eq!(
            fs::read_to_string(scratch.0.join(OLD_LOG_FILE)).unwrap(),
            format!("{line}{short}")
        );
        assert_eq!(fs::read_to_string(scratch.0.join(LOG_FILE)).unwrap(), line);
    }

    #[test]
    fn sessions_are_logged_without_their_contents() {
        use mklm_client::orchestrator::{
            LaunchError, LaunchFailure, RecoverySkip, RequestEnd, RequestReport,
        };
        use mklm_client::run_once::{RunOnceError, RunOnceOutcome};
        use mklm_client::session::{Notice, SessionEnd, SessionKind};
        use mklm_core::{ApplyOptions, LayoutChoice};

        let request = mklm_ipc::Request::SetLayout(mklm_ipc::SetLayoutRequest {
            instance_id: r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".into(),
            layout: LayoutChoice::Jis,
            apply: ApplyOptions::default(),
            expected: None,
        });
        assert_eq!(request_kind(&request), "SetLayout");
        assert_eq!(
            request_kind(&mklm_ipc::Request::Undo {
                apply: ApplyOptions::default()
            }),
            "Undo"
        );
        assert_eq!(
            notice_text(&Notice::Connected(SessionKind::Recovery)),
            "the helper is connected (Recovery)"
        );
        let outcome = crate::state::SessionOutcome {
            report: RequestReport {
                first: RequestEnd::Ended(SessionEnd::Lost {
                    exit_code: None,
                    detail: "the pipe closed".into(),
                    planned: true,
                    countdown: true,
                }),
                lost_needs_recovery: Some(Ok(true)),
                recovery: None,
                recovery_skipped: Some(RecoverySkip::Declined),
            },
            run_once: Ok(RunOnceOutcome::NotNeeded),
        };
        assert_eq!(
            session_end_text(&outcome),
            "the helper was lost (exit code None, journaled true, countdown true): the pipe closed; \
             recovery needed: true; recovery not started: Declined; post-reboot RunOnce: NotNeeded"
        );
        let declined = crate::state::SessionOutcome {
            report: RequestReport {
                first: RequestEnd::NotLaunched(LaunchError::Failed {
                    kind: LaunchFailure::HelperMissing,
                    message: "mklm-helper.exe is missing".into(),
                }),
                lost_needs_recovery: None,
                recovery: None,
                recovery_skipped: None,
            },
            run_once: Err(RunOnceError::Check("no journal".into())),
        };
        assert_eq!(
            session_end_text(&declined),
            "not launched: mklm-helper.exe is missing; post-reboot RunOnce failed: no journal"
        );
    }

    #[test]
    fn a_line_longer_than_the_limit_is_still_written() {
        let scratch = Scratch::new("long");
        let mut log = LogFile::new(scratch.0.clone(), 8);
        let line = format_line("t", Level::Info, "longer than the limit");
        log.append(&line).unwrap();
        assert_eq!(fs::read_to_string(scratch.0.join(LOG_FILE)).unwrap(), line);
        assert!(!scratch.0.join(OLD_LOG_FILE).exists());
    }
}
