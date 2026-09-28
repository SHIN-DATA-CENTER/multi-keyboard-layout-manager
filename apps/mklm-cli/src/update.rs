//! `mklm-cli update --check | --status [--json]` (design m5b D.14; J-4: the CLI checks and shows,
//! it never installs — the GUI does, or the installer run with `/S`).
//!
//! | Command | Exit codes |
//! |---|---|
//! | `update --check` | 0 up to date, 20 an update is available, 21 it must be installed by hand, 22 this build cannot use updates (`NotConfigured`), 6 an update is running now (try again in a minute), 1 the check failed, 2 usage |
//! | `update --status` | 0, 1, 2 |
//!
//! While an update runs (`Run` from `ready` on), the write commands and `update --check` end at
//! once with 6: MKLM's runner waits only 30 s for `mklm-cli.exe` to end, and a check may take
//! longer than that. The read-only commands, `update --status` included, are never held up
//! (they end within a second; FIX-VERIFICATION-14).
//!
//! Output is English, as the other commands'. The rendering is pure (tested with made-up
//! outcomes and records); `run` reads the machine and the network.

use std::fmt::Write as _;

use mklm_client::update::cache::{ClientState, ErrorClass};
use mklm_client::update::check::{CheckError, CheckOutcome};
use mklm_client::update::env::Availability;
use mklm_client::update::status::{outcome_diagnostic, phase_diagnostic};
use mklm_update::run::{RunPhase, RunView, UpdateResult};
use mklm_update::{Freshness, UpdateRefusal, VerifiedManifest};
use serde::Serialize;

/// `update --check` found the installed version to be the newest.
pub const UP_TO_DATE: i32 = 0;
/// A newer version can be installed with MKLM.
pub const UPDATE_AVAILABLE: i32 = 20;
/// A newer version needs the installer run by hand (`min_from_version`).
pub const MANUAL_REQUIRED: i32 = 21;
/// This build has no update keys.
pub const NOT_CONFIGURED: i32 = 22;
/// An update is running now (`write::exit_code::BLOCKED`).
pub const UPDATE_RUNNING: i32 = 6;
/// The check failed, or the records could not be read.
pub const FAILED: i32 = 1;

/// What `update` does.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(group(clap::ArgGroup::new("action").required(true).args(["check", "status"])))]
pub struct UpdateArgs {
    /// Check GitHub for a newer version (nothing is downloaded or installed).
    #[arg(long)]
    pub check: bool,
    /// Show the machine's update records and this user's last checks.
    #[arg(long)]
    pub status: bool,
    /// Print JSON.
    #[arg(long)]
    pub json: bool,
    /// Development builds only: check a local rehearsal server instead of GitHub
    /// (`http://127.0.0.1:<port>`, design m5b A.10, F.6).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    #[arg(long, value_name = "URL")]
    pub update_endpoint: Option<String>,
}

/// The message of an update that holds up a command (exit code 6).
pub const RUNNING_MESSAGE: &str = "MKLM is being updated. Try again in a minute.";

/// True when `run` is an update past `ready` (the runner waits for MKLM's programs to end).
pub fn update_running(run: &RunView) -> bool {
    matches!(
        run,
        RunView::InProgress { phase, .. } if !matches!(phase, RunPhase::Staging | RunPhase::Staged)
    )
}

/// What a command prints and its exit code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    /// `warning:` / `error:` lines.
    pub stderr: Vec<String>,
}

/// `update --check --json` (design m5b D.14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckDocument {
    pub installed: String,
    /// "up-to-date", "update-available" or "manual-required".
    pub status: &'static str,
    pub offered: String,
    /// "fresh" or "expired".
    pub freshness: &'static str,
    pub issued_at: u64,
    pub expires: u64,
    pub release_page: String,
    pub rollback_ignored: Option<RollbackIgnored>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollbackIgnored {
    pub issued_at: u64,
    pub seen: u64,
}

/// `YYYY-MM-DD` of Unix seconds, UTC.
pub fn utc_date(unix: u64) -> String {
    let (year, month, day) = civil_from_days((unix / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `YYYY-MM-DD HH:MM UTC` of Unix seconds.
pub fn utc_time(unix: u64) -> String {
    let seconds = unix % 86_400;
    format!(
        "{} {:02}:{:02} UTC",
        utc_date(unix),
        seconds / 3600,
        (seconds % 3600) / 60
    )
}

/// Days since 1970-01-01 → (year, month, day) (the proleptic Gregorian calendar).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The report of `update --check`: `result` of the check; `rollback` the older manifest this
/// check ignored when the cached one answered instead.
pub fn check_report(
    installed: &str,
    result: &Result<CheckOutcome, CheckError>,
    rollback: Option<(u64, u64)>,
    json: bool,
) -> Output {
    let mut out = Output::default();
    if let Some((issued_at, seen)) = rollback {
        out.stderr.push(format!(
            "warning: update information older than the one checked before was ignored (issued {}, \
             newest seen {}); it may have been withdrawn or replaced on the way",
            utc_date(issued_at),
            utc_date(seen)
        ));
    }
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            out.code = match error {
                CheckError::Unavailable(Availability::NotConfigured) => NOT_CONFIGURED,
                _ => FAILED,
            };
            out.stderr.push(match error {
                CheckError::Unavailable(Availability::NotConfigured) => {
                    "error: this build of MKLM has no update keys, so it cannot check for updates"
                        .to_string()
                }
                error => format!("error: {error}"),
            });
            return out;
        }
    };
    let verified = outcome.verified();
    let offered = verified.version.to_string();
    let release_page = mklm_update::release_page_url(&verified.version);
    let (code, status, text) = match outcome {
        CheckOutcome::UpToDate(_) => (
            UP_TO_DATE,
            "up-to-date",
            format!("MKLM is up to date ({installed})."),
        ),
        CheckOutcome::Available(_) => (
            UPDATE_AVAILABLE,
            "update-available",
            format!(
                "MKLM {offered} is available (installed {installed}). Open MKLM to install it."
            ),
        ),
        CheckOutcome::ManualRequired(_) => (
            MANUAL_REQUIRED,
            "manual-required",
            format!(
                "MKLM {offered} is available (installed {installed}), but it cannot be installed \
                 automatically from this version. Download the installer from {release_page}"
            ),
        ),
    };
    out.code = code;
    if verified.freshness == Freshness::Expired {
        out.stderr.push(format!(
            "warning: the update information expired on {}; no new version was published for a \
             long time, or older information is being served. Check {release_page}",
            utc_date(verified.expires)
        ));
    }
    if json {
        let document = CheckDocument {
            installed: installed.to_string(),
            status,
            offered,
            freshness: freshness(verified),
            issued_at: verified.issued_at,
            expires: verified.expires,
            release_page,
            rollback_ignored: rollback.map(|(issued_at, seen)| RollbackIgnored { issued_at, seen }),
        };
        out.stdout = crate::json::to_json(&document, true).unwrap_or_default();
    } else {
        out.stdout = format!(
            "{text}\nUpdate information: version {}, issued {}, expires {}.\n",
            verified.version,
            utc_date(verified.issued_at),
            utc_date(verified.expires)
        );
    }
    out
}

fn freshness(verified: &VerifiedManifest) -> &'static str {
    match verified.freshness {
        Freshness::Fresh => "fresh",
        Freshness::Expired => "expired",
    }
}

/// `update --status --json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusDocument<'a> {
    pub installed: &'a str,
    pub availability: String,
    /// "idle", "in-progress" or "interrupted".
    pub run: &'static str,
    pub run_phase: Option<RunPhase>,
    pub run_to_version: Option<String>,
    pub last_result: Option<&'a UpdateResult>,
    /// The version all three installed programs have; `null` when they differ or are missing.
    pub installed_consistently: Option<String>,
    pub last_check: Option<u64>,
    pub last_success: Option<u64>,
    pub last_failure_class: Option<ErrorClass>,
    pub last_failure: Option<String>,
    pub rollback_ignored: Option<RollbackIgnored>,
    pub warnings: &'a [String],
}

/// What `update --status` shows of the records (pure).
pub struct StatusInput<'a> {
    pub installed: &'a str,
    pub availability: &'a Availability,
    pub run: &'a RunView,
    pub last_result: Option<&'a UpdateResult>,
    pub consistent: Option<String>,
    pub client: &'a ClientState,
    pub warnings: &'a [String],
}

/// The report of `update --status`.
pub fn status_report(input: &StatusInput<'_>, json: bool) -> Output {
    let (run, phase, to_version) = match input.run {
        RunView::Idle => ("idle", None, None),
        RunView::InProgress { phase, to_version } => {
            ("in-progress", Some(*phase), Some(to_version.clone()))
        }
        RunView::Interrupted(record) => (
            "interrupted",
            Some(record.phase),
            Some(record.to_version.clone()),
        ),
    };
    let availability = match input.availability {
        Availability::Available => "available".to_string(),
        Availability::NotConfigured => "not-configured".to_string(),
        Availability::NotInstalledCopy { exe_dir, .. } => {
            format!("not-installed-copy ({})", exe_dir.display())
        }
        Availability::Unknown { detail } => format!("unknown ({detail})"),
    };
    let rollback = input
        .client
        .last_rollback
        .as_ref()
        .map(|note| RollbackIgnored {
            issued_at: note.issued_at,
            seen: note.seen,
        });
    let mut out = Output {
        stderr: input
            .warnings
            .iter()
            .map(|warning| format!("warning: {warning}"))
            .collect(),
        ..Output::default()
    };
    if json {
        let document = StatusDocument {
            installed: input.installed,
            availability,
            run,
            run_phase: phase,
            run_to_version: to_version,
            last_result: input.last_result,
            installed_consistently: input.consistent.clone(),
            last_check: input.client.last_check,
            last_success: input.client.last_success,
            last_failure_class: input.client.last_failure.as_ref().map(|f| f.class),
            last_failure: input
                .client
                .last_failure
                .as_ref()
                .map(|f| f.message_id.clone()),
            rollback_ignored: rollback,
            warnings: input.warnings,
        };
        out.stdout = crate::json::to_json(&document, true).unwrap_or_default();
        return out;
    }
    let mut text = String::new();
    let _ = writeln!(text, "Installed: MKLM {}", input.installed);
    let _ = writeln!(text, "Updates: {availability}");
    let _ = writeln!(
        text,
        "Installed programs: {}",
        match &input.consistent {
            Some(version) => format!("all three are {version}"),
            None => "their versions differ, or one is missing (run the installer again)".into(),
        }
    );
    let _ = match (run, phase, &to_version) {
        ("idle", _, _) => writeln!(text, "Update running: no"),
        (run, Some(phase), Some(version)) => {
            writeln!(
                text,
                "Update to {version}: {run} ({})",
                phase_diagnostic(phase)
            )
        }
        _ => Ok(()),
    };
    match input.last_result {
        Some(result) => {
            let _ = writeln!(
                text,
                "Last update: {} → {}, {} ({})",
                result.from_version,
                result.to_version,
                outcome_diagnostic(&result.outcome),
                utc_time(result.finished_at.0 / 1000)
            );
        }
        None => {
            let _ = writeln!(text, "Last update: none recorded");
        }
    }
    let time = |at: Option<u64>| at.map_or_else(|| "never".to_string(), utc_time);
    let _ = writeln!(text, "Last check: {}", time(input.client.last_check));
    let _ = writeln!(
        text,
        "Last successful check: {}",
        time(input.client.last_success)
    );
    if let Some(failure) = &input.client.last_failure {
        let _ = writeln!(
            text,
            "Last failure: {} ({}, failing since {})",
            failure.message_id,
            match failure.class {
                ErrorClass::Transient => "transient",
                ErrorClass::Structural => "structural",
            },
            utc_time(failure.first_at)
        );
    }
    if let Some(note) = &input.client.last_rollback {
        let _ = writeln!(
            text,
            "Older update information ignored: issued {}, newest seen {} (at {})",
            utc_date(note.issued_at),
            utc_date(note.seen),
            utc_time(note.at)
        );
    }
    out.stdout = text;
    out
}

/// Runs `update` (Windows).
#[cfg(windows)]
pub fn run(args: &UpdateArgs) -> anyhow::Result<i32> {
    use std::sync::atomic::AtomicBool;

    use anyhow::Context as _;
    use mklm_client::update::cache::UpdateCache;
    use mklm_client::update::check::{check, reverify_cached};
    use mklm_client::update::env::environment;
    use mklm_client::update::status::read_status;

    let installed = env!("CARGO_PKG_VERSION");
    let env = environment(installed, endpoints(args)?);
    let cache = UpdateCache::new(env.cache_dir.clone());
    let status = read_status(&env.install_dir, &cache);
    let output = if args.status {
        status_report(
            &StatusInput {
                installed,
                availability: &env.availability,
                run: &status.run,
                last_result: status.last_result.as_ref(),
                consistent: consistent(&status.install),
                client: &status.client,
                warnings: &status.warnings,
            },
            args.json,
        )
    } else if update_running(&status.run) {
        Output {
            code: UPDATE_RUNNING,
            stdout: String::new(),
            stderr: vec![RUNNING_MESSAGE.to_string()],
        }
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let mut rollback = None;
        let result = match transport(args) {
            Ok(mut transport) => {
                let result = check(
                    transport.as_mut(),
                    &env,
                    &cache,
                    &status.machine_trust,
                    None,
                    now,
                    &AtomicBool::new(false),
                );
                // An older manifest was served: what the cached one says, with a warning.
                if let Err(CheckError::Refused(UpdateRefusal::Rollback { issued_at, seen })) =
                    &result
                {
                    rollback = Some((*issued_at, *seen));
                    reverify_cached(&env, &cache, &status.machine_trust, None, now).or(result)
                } else {
                    result
                }
            }
            Err(error) => Err(error),
        };
        check_report(installed, &result, rollback, args.json)
    };
    for line in &output.stderr {
        eprintln!("{line}");
    }
    crate::print(&output.stdout).context("printing the update report")?;
    Ok(output.code)
}

/// The version all three installed programs have.
#[cfg(windows)]
fn consistent(install: &mklm_update::run::InstallState) -> Option<String> {
    install
        .consistent_version()
        .map(|version| version.to_string())
}

/// The release endpoints: GitHub; in development builds maybe `--update-endpoint`.
#[cfg(windows)]
fn endpoints(args: &UpdateArgs) -> anyhow::Result<mklm_update::url::Endpoints> {
    #[cfg(all(debug_assertions, mklm_update_dev))]
    if let Some(base) = &args.update_endpoint {
        return mklm_update::url::Endpoints::loopback(base)
            .map_err(|error| anyhow::anyhow!("--update-endpoint: {error}"));
    }
    let _ = args;
    Ok(mklm_update::url::Endpoints::production())
}

/// WinHTTP with the automatic proxy (design m5b A.7); without a proxy for the rehearsal server.
#[cfg(windows)]
fn transport(args: &UpdateArgs) -> Result<Box<dyn mklm_update::fetch::Transport>, CheckError> {
    use mklm_update::winhttp::WinHttpTransport;

    let agent = mklm_update::user_agent(
        env!("CARGO_PKG_VERSION"),
        mklm_update::Arch::of_this_build(),
    );
    #[cfg(all(debug_assertions, mklm_update_dev))]
    if args.update_endpoint.is_some() {
        return WinHttpTransport::new_without_proxy(&agent)
            .map(|transport| Box::new(transport) as Box<dyn mklm_update::fetch::Transport>)
            .map_err(|error| CheckError::Fetch(mklm_update::fetch::FetchError::Transport(error)));
    }
    let _ = args;
    WinHttpTransport::new(&agent)
        .map(|transport| Box::new(transport) as Box<dyn mklm_update::fetch::Transport>)
        .map_err(|error| CheckError::Fetch(mklm_update::fetch::FetchError::Transport(error)))
}

/// The early end of a command while an update runs (design m5b D.14): the write commands and
/// `update --check`. Reads `Run` (Users may); a record that cannot be read holds nothing up.
#[cfg(windows)]
pub fn blocked_by_update() -> bool {
    mklm_client::update::status::current_run().is_some_and(|(_, view)| update_running(&view))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use mklm_client::update::cache::{CheckFailure, RollbackNote};
    use mklm_client::update::check::Offer;
    use mklm_core::Timestamp;
    use mklm_update::fetch::FetchError;
    use mklm_update::run::{FileHolder, NotInstalledReason, ProgramKind, RunId, UpdateOutcome};
    use mklm_update::{
        Arch, KeyFingerprint, KeyId, KeyRole, OfferKind, SelectedAsset, Sha256Digest,
        SignatureSlot, Version,
    };
    use serde_json::Value;

    use super::*;

    fn verified(version: &str, freshness: Freshness) -> VerifiedManifest {
        let version = Version::parse(version).unwrap();
        VerifiedManifest {
            asset: SelectedAsset {
                arch: Arch::X64,
                name: mklm_update::installer_name(&version, Arch::X64),
                size: 6_291_456,
                sha256: Sha256Digest([3; 32]),
            },
            version,
            issued_at: 1_792_022_400,
            expires: 1_807_574_400,
            signer: KeyId([1; 8]),
            signer_role: KeyRole::Primary,
            signer_fingerprint: KeyFingerprint([2; 32]),
            signer_is_dev: false,
            key_ids: vec![KeyId([1; 8])],
            revoked: BTreeMap::new(),
            min_from_version: None,
            freshness,
            offer: OfferKind::Newer,
        }
    }

    fn available(freshness: Freshness) -> CheckOutcome {
        CheckOutcome::Available(Offer {
            verified: verified("0.2.1", freshness),
            manifest: Vec::new(),
            signature: Vec::new(),
            slot: SignatureSlot::Main,
            skipped: false,
            downloaded: None,
        })
    }

    /// The exit codes of `update --check` (design m5b D.14; F.4).
    #[test]
    fn check_exit_codes() {
        let cases: [(Result<CheckOutcome, CheckError>, i32); 7] = [
            (
                Ok(CheckOutcome::UpToDate(verified("0.2.0", Freshness::Fresh))),
                0,
            ),
            (Ok(available(Freshness::Fresh)), 20),
            (
                Ok(CheckOutcome::ManualRequired(verified(
                    "0.2.1",
                    Freshness::Fresh,
                ))),
                21,
            ),
            (
                Err(CheckError::Unavailable(Availability::NotConfigured)),
                22,
            ),
            (Err(CheckError::Fetch(FetchError::NotFound)), 1),
            (Err(CheckError::Refused(UpdateRefusal::BadSignature)), 1),
            (Err(CheckError::Cache("disk full".into())), 1),
        ];
        for (result, code) in cases {
            assert_eq!(
                check_report("0.2.0", &result, None, false).code,
                code,
                "{result:?}"
            );
            assert_eq!(check_report("0.2.0", &result, None, true).code, code);
        }
        let text = check_report("0.2.0", &Ok(available(Freshness::Fresh)), None, false);
        assert!(
            text.stdout.starts_with(
                "MKLM 0.2.1 is available (installed 0.2.0). Open MKLM to install it.\n"
            )
        );
        assert!(text.stderr.is_empty());
        let up = check_report(
            "0.2.0",
            &Ok(CheckOutcome::UpToDate(verified("0.2.0", Freshness::Fresh))),
            None,
            false,
        );
        assert!(up.stdout.starts_with("MKLM is up to date (0.2.0).\n"));
        assert!(up.stdout.contains("issued 2026-10-15, expires 2027-04-13"));
    }

    /// The JSON of D.14, with `issued_at` (RED-TEAM-1) and the warnings.
    #[test]
    fn check_json_and_warnings() {
        let out = check_report(
            "0.2.0",
            &Ok(available(Freshness::Expired)),
            Some((1_791_158_400, 1_792_022_400)),
            true,
        );
        let value: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(value["installed"], "0.2.0");
        assert_eq!(value["status"], "update-available");
        assert_eq!(value["offered"], "0.2.1");
        assert_eq!(value["freshness"], "expired");
        assert_eq!(value["issued_at"], 1_792_022_400u64);
        assert_eq!(value["expires"], 1_807_574_400u64);
        assert_eq!(
            value["release_page"],
            "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/tag/v0.2.1"
        );
        assert_eq!(value["rollback_ignored"]["seen"], 1_792_022_400u64);
        // One line each: the expired information and the ignored older one.
        assert_eq!(out.stderr.len(), 2);
        assert!(
            out.stderr
                .iter()
                .any(|line| line.contains("expired on 2027-04-13"))
        );
        assert!(out.stderr.iter().any(|line| line.contains("older")));
        let fresh = check_report("0.2.0", &Ok(available(Freshness::Fresh)), None, true);
        let value: Value = serde_json::from_str(&fresh.stdout).unwrap();
        assert_eq!(value["rollback_ignored"], Value::Null);
        assert_eq!(value["freshness"], "fresh");
    }

    /// Only an update past `ready` holds commands up (design m5b D.14; FIX-VERIFICATION-14).
    #[test]
    fn only_a_running_update_holds_commands_up() {
        for (phase, running) in [
            (RunPhase::Staging, false),
            (RunPhase::Staged, false),
            (RunPhase::Ready, true),
            (RunPhase::Waiting, true),
            (RunPhase::Installing, true),
            (RunPhase::Finishing, true),
        ] {
            let view = RunView::InProgress {
                phase,
                to_version: "0.2.1".into(),
            };
            assert_eq!(update_running(&view), running, "{phase:?}");
        }
        assert!(!update_running(&RunView::Idle));
        assert_eq!(
            RUNNING_MESSAGE,
            "MKLM is being updated. Try again in a minute."
        );
        assert_eq!(UPDATE_RUNNING, crate::write::exit_code::BLOCKED);
    }

    fn result(outcome: UpdateOutcome) -> UpdateResult {
        UpdateResult {
            schema: 1,
            run_id: RunId::parse("0.2.1-3f9a0c2b7d1e4a65").unwrap(),
            from_version: "0.2.0".into(),
            to_version: "0.2.1".into(),
            arch: Arch::X64,
            finished_at: Timestamp(1_792_022_490_000),
            outcome,
            installer_exit: None,
            installed_version: Some("0.2.0".into()),
            gui_relaunch_attempted: true,
        }
    }

    #[test]
    fn the_status_report() {
        let client = ClientState {
            last_check: Some(1_792_026_000),
            last_success: Some(1_792_022_400),
            last_failure: Some(CheckFailure {
                class: ErrorClass::Transient,
                message_id: "upd-net".into(),
                first_at: 1_792_024_000,
                at: 1_792_026_000,
            }),
            last_rollback: Some(RollbackNote {
                issued_at: 1_791_158_400,
                seen: 1_792_022_400,
                at: 1_792_026_000,
            }),
            ..ClientState::default()
        };
        let last = result(UpdateOutcome::NotInstalled(
            NotInstalledReason::FilesInUse {
                programs: vec![ProgramKind::Gui],
                holders: vec![FileHolder {
                    pid: 7120,
                    session_id: 2,
                    name: "powershell.exe".into(),
                }],
            },
        ));
        let warnings = vec!["reading the boot ID failed: x".to_string()];
        let input = StatusInput {
            installed: "0.2.0",
            availability: &Availability::NotInstalledCopy {
                exe_dir: PathBuf::from(r"D:\dev"),
                install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
            },
            run: &RunView::InProgress {
                phase: RunPhase::Installing,
                to_version: "0.2.1".into(),
            },
            last_result: Some(&last),
            consistent: Some("0.2.0".into()),
            client: &client,
            warnings: &warnings,
        };
        let text = status_report(&input, false);
        assert_eq!(text.code, 0);
        assert_eq!(text.stderr, vec!["warning: reading the boot ID failed: x"]);
        for line in [
            "Installed: MKLM 0.2.0",
            "Update to 0.2.1: in-progress (installing)",
            "Last update: 0.2.0 → 0.2.1, not installed: MKLM's files were in use ([Gui]; powershell.exe (PID 7120, session 2))",
            "Last check: 2026-10-15 01:00 UTC",
            "Last failure: upd-net (transient, failing since 2026-10-15 00:26 UTC)",
            "Older update information ignored: issued 2026-10-05, newest seen 2026-10-15",
        ] {
            assert!(text.stdout.contains(line), "{line}\n{}", text.stdout);
        }
        let json = status_report(&input, true);
        let value: Value = serde_json::from_str(&json.stdout).unwrap();
        assert_eq!(value["run"], "in-progress");
        assert_eq!(value["run_phase"], "installing");
        assert_eq!(value["last_result"]["outcome"]["kind"], "not-installed");
        assert_eq!(value["last_failure_class"], "transient");
        assert_eq!(value["installed_consistently"], "0.2.0");
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_792_022_400), "2026-10-15");
        assert_eq!(utc_date(1_807_574_400), "2027-04-13");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_time(1_792_026_000), "2026-10-15 01:00 UTC");
    }
}
