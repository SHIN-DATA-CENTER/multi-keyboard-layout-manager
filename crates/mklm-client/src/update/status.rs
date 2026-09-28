//! The machine's update records and the user's, read unelevated (design m5b D.6, D.13, D.14,
//! E.2): `Trust`, `Run` and `LastResult` of `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` (Users
//! may read them; only the helper writes them), the build IDs of the three installed
//! executables, and the user's cache. What cannot be read reads as nothing, with an English
//! warning.

use std::path::Path;

use mklm_core::{BootId, Liveness, ProcessIdentity};
use mklm_update::TrustState;
use mklm_update::run::{InstallState, RunPhase, RunRecord, RunView, UpdateResult, classify_run};
use mklm_win::update_store::{RawUpdateStore, read_update_store};

use crate::update::cache::{ClientState, UpdateCache};
use crate::update::stage::HandOffProbe;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub machine_trust: TrustState,
    pub run: RunView,
    pub last_result: Option<UpdateResult>,
    /// `InstallState::from_build_ids(update_dir::read_build_ids(install_dir))`.
    pub install: InstallState,
    pub client: ClientState,
    /// English diagnostics of what could not be read (the fields then hold defaults).
    pub warnings: Vec<String>,
}

/// Unelevated: `read_update_store`, `classify_run` (boot ID, process liveness), the build IDs of
/// the three executables in `install_dir`, the user cache.
pub fn read_status(install_dir: &Path, cache: &UpdateCache) -> UpdateStatus {
    let mut warnings = Vec::new();
    let raw = read_update_store().unwrap_or_else(|error| {
        warnings.push(format!(
            "reading the machine's update records failed: {error}"
        ));
        RawUpdateStore::default()
    });
    let records = parse_records(&raw, &mut warnings);
    let boot = mklm_win::session::boot_id().map_err(|error| error.to_string());
    let run = classify_record(
        records.run.as_ref(),
        boot,
        &mklm_win::proc_identity::process_liveness,
        &mut warnings,
    );
    UpdateStatus {
        machine_trust: records.trust,
        run,
        last_result: records.last_result,
        install: InstallState::from_build_ids(mklm_win::update_dir::read_build_ids(install_dir)),
        client: cache.load_state(),
        warnings,
    }
}

/// The three values, parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Records {
    pub trust: TrustState,
    pub run: Option<RunRecord>,
    pub last_result: Option<UpdateResult>,
}

/// Parses what was read; a value that cannot be parsed reads as absent, with a warning.
pub(crate) fn parse_records(raw: &RawUpdateStore, warnings: &mut Vec<String>) -> Records {
    let mut records = Records::default();
    if let Some(text) = &raw.trust {
        match TrustState::parse(text) {
            Ok(trust) => records.trust = trust,
            Err(error) => warnings.push(format!("the machine's Trust record: {error}")),
        }
    }
    if let Some(text) = &raw.run {
        match RunRecord::from_json(text) {
            Ok(run) => records.run = Some(run),
            Err(error) => warnings.push(format!("the machine's Run record: {error}")),
        }
    }
    if let Some(text) = &raw.last_result {
        match UpdateResult::from_json(text) {
            Ok(result) => records.last_result = Some(result),
            Err(error) => warnings.push(format!("the machine's LastResult record: {error}")),
        }
    }
    records
}

/// `classify_run` with this boot; when the boot ID cannot be read, the record's own boot is
/// assumed, so that the owners' liveness alone decides (a running update is never taken for an
/// interrupted one because of a read error).
pub(crate) fn classify_record(
    run: Option<&RunRecord>,
    boot: Result<BootId, String>,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
    warnings: &mut Vec<String>,
) -> RunView {
    let boot = match (boot, run) {
        (Ok(boot), _) => boot,
        (Err(error), Some(record)) => {
            warnings.push(format!("reading the boot ID failed: {error}"));
            record.boot_id
        }
        (Err(_), None) => return RunView::Idle,
    };
    classify_run(run, boot, liveness)
}

/// `Run` now: the record and how it stands (`None` when there is none or it cannot be read).
/// For the front ends' start (design m5b D.13 step 1, D.14's early end).
pub fn current_run() -> Option<(RunRecord, RunView)> {
    let raw = read_update_store().ok()?;
    let record = RunRecord::from_json(raw.run.as_deref()?).ok()?;
    let mut warnings = Vec::new();
    let view = classify_record(
        Some(&record),
        mklm_win::session::boot_id().map_err(|error| error.to_string()),
        &mklm_win::proc_identity::process_liveness,
        &mut warnings,
    );
    Some((record, view))
}

/// A phase in English (diagnostics, the CLI).
pub fn phase_diagnostic(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Staging => "receiving the installer",
        RunPhase::Staged => "installer received",
        RunPhase::Ready => "ready",
        RunPhase::Waiting => "waiting for MKLM's programs to end",
        RunPhase::Installing => "installing",
        RunPhase::Finishing => "checking the result",
        RunPhase::Done => "done",
    }
}

/// An update's outcome in English (diagnostics, the CLI): what `LastResult` says.
pub fn outcome_diagnostic(outcome: &mklm_update::run::UpdateOutcome) -> String {
    use mklm_update::run::{FailedReason, NotInstalledReason, UpdateOutcome};
    let holders = |holders: &[mklm_update::run::FileHolder]| {
        if holders.is_empty() {
            return "holders unknown".to_string();
        }
        holders
            .iter()
            .map(|holder| {
                format!(
                    "{} (PID {}, session {})",
                    holder.name, holder.pid, holder.session_id
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    match outcome {
        UpdateOutcome::Installed => "installed".into(),
        UpdateOutcome::Interrupted { phase } => {
            format!("interrupted while {}", phase_diagnostic(*phase))
        }
        UpdateOutcome::Failed(reason) => match reason {
            FailedReason::InstallerTimedOut => "failed: the installer did not end in time".into(),
            FailedReason::Inconsistent => {
                "failed: the installed programs are of different versions".into()
            }
            FailedReason::UnexpectedVersion { found } => {
                format!("failed: version {found} is installed")
            }
        },
        UpdateOutcome::NotInstalled(reason) => {
            let why = match reason {
                NotInstalledReason::Refused(refusal) => refusal.to_string(),
                NotInstalledReason::CallerDidNotExit => "the MKLM that asked did not end".into(),
                NotInstalledReason::InstanceBusy { sessions } => {
                    format!("MKLM was busy in session(s) {sessions:?}")
                }
                NotInstalledReason::ProgramsStillRunning {
                    programs,
                    holders: list,
                } => format!(
                    "MKLM programs were still running ({programs:?}; {})",
                    holders(list)
                ),
                NotInstalledReason::FilesInUse {
                    programs,
                    holders: list,
                } => format!("MKLM's files were in use ({programs:?}; {})", holders(list)),
                NotInstalledReason::DiskFull { needed, available } => {
                    format!("not enough disk space ({needed} bytes needed, {available} free)")
                }
                NotInstalledReason::SessionEnding => "the Windows session was ending".into(),
                NotInstalledReason::InstalledVersionChanged { found } => format!(
                    "another version was installed meanwhile ({})",
                    found.as_deref().unwrap_or("unknown")
                ),
                NotInstalledReason::InstallerNotStarted { code } => {
                    format!("the installer did not start (Windows error {code})")
                }
                NotInstalledReason::InstallerRefused { exit } => {
                    format!("the installer refused ({exit:?})")
                }
                NotInstalledReason::InstallerExit { code } => {
                    format!("the installer ended with code {code}")
                }
            };
            format!("not installed: {why}")
        }
    }
}

/// `HandOffProbe` over `read_update_store` for this process (`current_process_identity`).
#[derive(Debug)]
pub struct RunRecordProbe {
    /// This process; `None` when it could not be read (then nothing is ever a hand-off).
    me: Option<ProcessIdentity>,
}

impl RunRecordProbe {
    pub fn new() -> RunRecordProbe {
        RunRecordProbe {
            me: mklm_win::proc_identity::current_process_identity().ok(),
        }
    }
}

impl Default for RunRecordProbe {
    fn default() -> RunRecordProbe {
        RunRecordProbe::new()
    }
}

impl HandOffProbe for RunRecordProbe {
    fn handed_off(&mut self) -> Option<(String, String)> {
        let me = self.me.as_ref()?;
        let raw = read_update_store().ok()?;
        let run = RunRecord::from_json(raw.run.as_deref()?).ok()?;
        handed_off_to(&run, me, &mklm_win::proc_identity::process_liveness)
    }
}

/// `run` took over for `me`: `Run.caller` is `me`, the phase is ready or waiting, and the runner
/// lives (design m5b E.4; RELIABILITY-5).
pub(crate) fn handed_off_to(
    run: &RunRecord,
    me: &ProcessIdentity,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
) -> Option<(String, String)> {
    let taken_over = run.caller.as_ref() == Some(me)
        && matches!(run.phase, RunPhase::Ready | RunPhase::Waiting)
        && run
            .runner
            .as_ref()
            .is_some_and(|runner| liveness(runner) == Liveness::Alive);
    taken_over.then(|| (run.run_id.as_str().to_string(), run.to_version.clone()))
}

#[cfg(test)]
mod tests {
    use mklm_core::Timestamp;
    use mklm_update::Arch;
    use mklm_update::run::RunId;

    use super::*;

    fn identity(pid: u32) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            creation_time: 134_041_234_567_890_123 + u64::from(pid),
        }
    }

    fn record(phase: RunPhase) -> RunRecord {
        RunRecord {
            schema: 1,
            run_id: RunId::parse("0.2.1-3f9a0c2b7d1e4a65").unwrap(),
            from_version: "0.2.0".into(),
            to_version: "0.2.1".into(),
            arch: Arch::X64,
            phase,
            boot_id: BootId(7),
            started_at: Timestamp(1),
            phase_at: Timestamp(2),
            caller: Some(identity(8532)),
            caller_session: Some(1),
            stager: identity(9120),
            runner: Some(identity(9344)),
            installer: None,
        }
    }

    #[test]
    fn the_runner_took_over_for_this_process() {
        let alive = |_: &ProcessIdentity| Liveness::Alive;
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        let me = identity(8532);
        for phase in [RunPhase::Ready, RunPhase::Waiting] {
            assert_eq!(
                handed_off_to(&record(phase), &me, &alive),
                Some(("0.2.1-3f9a0c2b7d1e4a65".into(), "0.2.1".into()))
            );
        }
        // Not yet ready, or already installing: not what a lost `HandedOff` looks like.
        for phase in [RunPhase::Staging, RunPhase::Staged, RunPhase::Installing] {
            assert_eq!(handed_off_to(&record(phase), &me, &alive), None);
        }
        // Another caller, or a dead runner.
        assert_eq!(
            handed_off_to(&record(RunPhase::Ready), &identity(1), &alive),
            None
        );
        assert_eq!(handed_off_to(&record(RunPhase::Ready), &me, &dead), None);
        let mut no_runner = record(RunPhase::Ready);
        no_runner.runner = None;
        assert_eq!(handed_off_to(&no_runner, &me, &alive), None);
    }

    #[test]
    fn unreadable_records_read_as_absent() {
        let mut warnings = Vec::new();
        let records = parse_records(&RawUpdateStore::default(), &mut warnings);
        assert_eq!(records, Records::default());
        assert!(warnings.is_empty());
        let raw = RawUpdateStore {
            trust: Some("{bad".into()),
            run: Some("{bad".into()),
            last_result: Some("{bad".into()),
        };
        let records = parse_records(&raw, &mut warnings);
        assert_eq!(records, Records::default());
        assert_eq!(warnings.len(), 3);
        // No record: idle, whatever the boot.
        let mut warnings = Vec::new();
        assert_eq!(
            classify_record(None, Err("x".into()), &|_| Liveness::Alive, &mut warnings),
            RunView::Idle
        );
        assert!(warnings.is_empty());
    }
}
