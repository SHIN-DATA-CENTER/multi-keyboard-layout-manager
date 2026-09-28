//! One update run (design m5b D.6, D.7, D.9.1, D.12, D.13): its ID, the machine records `Run` and
//! `LastResult`, the NSIS exit codes, the outcome rules, and the waits and deadlines.
//!
//! WP-0 writes the types and the grammar ([`RunId`], [`classify_installer_exit`],
//! [`InstallerExit::leaves_old_files`], [`nsis_exit`]); WP-H the rules.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::fmt::Write as _;

use mklm_core::{BootId, Liveness, ProcessIdentity, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Version;
use crate::manifest::Arch;
use crate::refusal::UpdateRefusal;
use crate::state::{StateError, skeleton};
use crate::version::MAX_VERSION_PART;

/// `<major>.<minor>.<patch>-<16 lower-case hex digits>`, each number 0..=65535 without a
/// leading zero. Folder name and record key of one update.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RunId(String);

impl RunId {
    /// `version` is the verified target version (`X.Y.Z`, each part at most 65535: design m5b A.2);
    /// a pre-release or build part is not part of the ID.
    pub fn new(version: &Version, random: [u8; 8]) -> RunId {
        debug_assert!(
            [version.major, version.minor, version.patch]
                .iter()
                .all(|&part| part <= MAX_VERSION_PART),
            "{version} is not a release version"
        );
        let mut text = format!("{}.{}.{}-", version.major, version.minor, version.patch);
        for byte in random {
            let _ = write!(text, "{byte:02x}");
        }
        RunId(text)
    }

    pub fn parse(text: &str) -> Result<RunId, UpdateRefusal> {
        let bad = || UpdateRefusal::Internal {
            detail: format!("{text:?} is not an update run ID"),
        };
        let (version, suffix) = text.split_once('-').ok_or_else(bad)?;
        let mut parts = 0;
        for part in version.split('.') {
            parts += 1;
            if version_part(part).is_none() {
                return Err(bad());
            }
        }
        let hex = suffix.len() == 16
            && suffix
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if parts != 3 || !hex {
            return Err(bad());
        }
        Ok(RunId(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn version(&self) -> Version {
        let version = self.0.split_once('-').map_or("", |(version, _)| version);
        let mut parts = version
            .split('.')
            .map(|part| version_part(part).unwrap_or(0));
        let mut next = || parts.next().unwrap_or(0);
        Version::new(next(), next(), next())
    }
}

/// `0` or `[1-9][0-9]{0,4}`, at most [`MAX_VERSION_PART`].
fn version_part(text: &str) -> Option<u64> {
    let digits = text.as_bytes();
    let well_formed = !digits.is_empty()
        && digits.len() <= 5
        && digits.iter().all(u8::is_ascii_digit)
        && (digits == b"0" || digits[0] != b'0');
    if !well_formed {
        return None;
    }
    text.parse::<u64>()
        .ok()
        .filter(|&value| value <= MAX_VERSION_PART)
}

impl TryFrom<String> for RunId {
    type Error = UpdateRefusal;

    fn try_from(text: String) -> Result<RunId, UpdateRefusal> {
        RunId::parse(&text)
    }
}

impl From<RunId> for String {
    fn from(id: RunId) -> String {
        id.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunPhase {
    Staging,
    Staged,
    Ready,
    Waiting,
    Installing,
    Finishing,
    Done,
}

/// `HKLM\…\MKLM\Update\Run` (design m5b D.6, D.12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub schema: u32, // 1
    pub run_id: RunId,
    pub from_version: String,
    pub to_version: String,
    pub arch: Arch,
    pub phase: RunPhase,
    pub boot_id: BootId,
    pub started_at: Timestamp,
    pub phase_at: Timestamp,
    /// The GUI that asked (the pipe server of H1).
    pub caller: Option<ProcessIdentity>,
    pub caller_session: Option<u32>,
    /// H1.
    pub stager: ProcessIdentity,
    /// H2, from `ready` on.
    pub runner: Option<ProcessIdentity>,
    /// The NSIS process, written together with `phase = installing` while it is still suspended
    /// (RELIABILITY-4).
    pub installer: Option<ProcessIdentity>,
}

impl RunRecord {
    pub fn to_json(&self) -> String {
        String::new() // Skeleton (M5b): WP-H
    }

    pub fn from_json(text: &str) -> Result<RunRecord, StateError> {
        Err(skeleton()) // Skeleton (M5b): WP-H
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgramKind {
    Gui,
    Cli,
    Helper,
}

/// NSIS exit codes of the installer (design m5b D.9.1); mirrored by `!define MKLM_EXIT_*` in
/// mklm.nsi. 25 (`MKLM_EXIT_BAD_INSTALL_DIR`) is the uninstaller's only and is not here.
pub mod nsis_exit {
    pub const OS_TOO_OLD: u32 = 20;
    pub const WRONG_ARCH: u32 = 21;
    pub const HELPER_RUNNING: u32 = 22;
    pub const CLI_RUNNING: u32 = 23;
    pub const GUI_RUNNING: u32 = 24;
    pub const FILES_IN_USE: u32 = 26;
    pub const FILE_WRITE: u32 = 27;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallerExit {
    Success,       // 0
    UserCancelled, // 1
    ScriptAborted, // 2
    OsTooOld,      // 20
    WrongArch,     // 21
    HelperRunning, // 22
    CliRunning,    // 23
    GuiRunning,    // 24
    FilesInUse,    // 26
    FileWrite,     // 27
    Other(u32),    // 25 included
}

impl InstallerExit {
    /// 20..=24, 26, 27: NSIS guarantees nothing was replaced (renamed from `is_refusal`).
    pub fn leaves_old_files(self) -> bool {
        matches!(
            self,
            InstallerExit::OsTooOld
                | InstallerExit::WrongArch
                | InstallerExit::HelperRunning
                | InstallerExit::CliRunning
                | InstallerExit::GuiRunning
                | InstallerExit::FilesInUse
                | InstallerExit::FileWrite
        )
    }
}

pub fn classify_installer_exit(code: u32) -> InstallerExit {
    match code {
        0 => InstallerExit::Success,
        1 => InstallerExit::UserCancelled,
        2 => InstallerExit::ScriptAborted,
        nsis_exit::OS_TOO_OLD => InstallerExit::OsTooOld,
        nsis_exit::WRONG_ARCH => InstallerExit::WrongArch,
        nsis_exit::HELPER_RUNNING => InstallerExit::HelperRunning,
        nsis_exit::CLI_RUNNING => InstallerExit::CliRunning,
        nsis_exit::GUI_RUNNING => InstallerExit::GuiRunning,
        nsis_exit::FILES_IN_USE => InstallerExit::FilesInUse,
        nsis_exit::FILE_WRITE => InstallerExit::FileWrite,
        other => InstallerExit::Other(other),
    }
}

/// Build IDs (VERSIONINFO `MKLMBuildId`) of the three installed executables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallState {
    pub gui: Option<String>,
    pub cli: Option<String>,
    pub helper: Option<String>,
}

impl InstallState {
    /// From `mklm_win::update_dir::read_build_ids` (order: gui, cli, helper). The one conversion
    /// H2 and `read_status` share (OPS-UX-TEST-12).
    pub fn from_build_ids(ids: [Option<String>; 3]) -> InstallState {
        let [gui, cli, helper] = ids;
        InstallState { gui, cli, helper }
    }

    /// The version part of the build ID when all three exist and are equal.
    pub fn consistent_version(&self) -> Option<Version> {
        None // Skeleton (M5b): WP-H
    }
}

/// A process that kept the update from running (RED-TEAM-3): for the administrator, in the
/// technical details and `update --status`. No user SID (`LastResult` is readable by Users).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileHolder {
    pub pid: u32,
    pub session_id: u32,
    /// Executable file name (Restart Manager's `strAppName` or the image's file name), at most
    /// 260 characters, control characters removed.
    pub name: String,
}

/// A running installed MKLM program (design m5b D.8 step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProgram {
    pub identity: ProcessIdentity,
    pub kind: ProgramKind,
    pub session_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum NotInstalledReason {
    Refused(UpdateRefusal),
    CallerDidNotExit,
    /// The sessions whose MKLM answered `busy` (RED-TEAM-3).
    InstanceBusy {
        #[serde(default)]
        sessions: Vec<u32>,
    },
    ProgramsStillRunning {
        programs: Vec<ProgramKind>,
        #[serde(default)]
        holders: Vec<FileHolder>,
    },
    /// Opened by another process without delete sharing (SECURITY-10); `holders` from the
    /// Restart Manager, empty when it could not tell (RED-TEAM-3).
    FilesInUse {
        programs: Vec<ProgramKind>,
        #[serde(default)]
        holders: Vec<FileHolder>,
    },
    DiskFull {
        needed: u64,
        available: u64,
    },
    /// The session began to end before the installer was started (RELIABILITY-3).
    SessionEnding,
    InstalledVersionChanged {
        found: Option<String>,
    },
    InstallerNotStarted {
        code: u32,
    },
    InstallerRefused {
        exit: InstallerExit,
    },
    InstallerExit {
        code: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum FailedReason {
    InstallerTimedOut,
    Inconsistent,
    UnexpectedVersion { found: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
pub enum UpdateOutcome {
    Installed,
    NotInstalled(NotInstalledReason),
    Failed(FailedReason),
    Interrupted { phase: RunPhase },
}

/// `HKLM\…\MKLM\Update\LastResult` (design m5b D.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateResult {
    pub schema: u32, // 1
    pub run_id: RunId,
    pub from_version: String,
    pub to_version: String,
    pub arch: Arch,
    pub finished_at: Timestamp,
    pub outcome: UpdateOutcome,
    pub installer_exit: Option<u32>,
    /// `InstallState::consistent_version` afterwards; `None` when inconsistent or unknown.
    pub installed_version: Option<String>,
    pub gui_relaunch_attempted: bool,
}

impl UpdateResult {
    pub fn to_json(&self) -> String {
        String::new() // Skeleton (M5b): WP-H
    }

    pub fn from_json(text: &str) -> Result<UpdateResult, StateError> {
        Err(skeleton()) // Skeleton (M5b): WP-H
    }
}

/// The table of design m5b D.13.
pub fn decide_outcome(
    from: &Version,
    to: &Version,
    exit: Option<u32>,
    timed_out: bool,
    after: &InstallState,
) -> UpdateOutcome {
    UpdateOutcome::Failed(FailedReason::Inconsistent) // Skeleton (M5b): WP-H
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunView {
    Idle,
    InProgress { phase: RunPhase, to_version: String },
    Interrupted(RunRecord),
}

/// `Run` as the GUI, the CLI and the helper see it: `InProgress` when the owner of the phase is
/// alive in this boot — `stager` up to `staged`, `runner` for `ready` and `waiting`, `runner` OR
/// `installer` for `installing` and `finishing` (RELIABILITY-4); otherwise (boot changed, owners
/// dead or unknown) `Interrupted`. `Done` or no record → `Idle`.
pub fn classify_run(
    run: Option<&RunRecord>,
    current_boot: BootId,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
) -> RunView {
    RunView::Idle // Skeleton (M5b): WP-H
}

/// The `LastResult` for an interrupted record (`UpdateOutcome::Interrupted`, the installed
/// version from `after`).
pub fn interrupted_result(
    record: &RunRecord,
    now: Timestamp,
    after: &InstallState,
) -> UpdateResult {
    UpdateResult {
        schema: 1,
        run_id: record.run_id.clone(),
        from_version: record.from_version.clone(),
        to_version: record.to_version.clone(),
        arch: record.arch,
        finished_at: now,
        outcome: UpdateOutcome::Interrupted {
            phase: record.phase,
        },
        installer_exit: None,
        installed_version: None, // Skeleton (M5b): WP-H (`after.consistent_version()`)
        gui_relaunch_attempted: false,
    }
}

/// What another MKLM's single-instance pipe answered (mapped from `mklm_win::instance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceAnswer {
    Quit,
    Busy,
    NotOurs,
    NoAnswer,
}

/// Free space the update needs (design m5b D.4 step 4, D.7 step 12).
pub mod space {
    pub const STAGING_MARGIN: u64 = 16 * 1024 * 1024;
    pub const INSTALL_FACTOR: u64 = 4;
    pub const INSTALL_MARGIN: u64 = 64 * 1024 * 1024;
}

/// Waits and deadlines of the update run (design m5b D.4, D.7, D.8, E.1–E.4).
pub mod timing {
    use std::time::Duration;
    pub const LOCK_WAIT_STAGER: Duration = Duration::from_secs(10);
    pub const CHUNK_WAIT: Duration = Duration::from_secs(30);
    pub const STAGE_TOTAL: Duration = Duration::from_secs(10 * 60);
    /// H1: H2 up to `ready` (RELIABILITY-5: was 20 s).
    pub const READY_WAIT: Duration = Duration::from_secs(120);
    /// H1: after TerminateProcess of H2, before deleting its folder.
    pub const RUNNER_KILL_WAIT: Duration = Duration::from_secs(5);
    pub const STAGER_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const LOCK_WAIT_RUNNER: Duration = Duration::from_secs(60);
    pub const CALLER_EXIT_WAIT: Duration = Duration::from_secs(30);
    /// One instance pipe (connect, send, reply).
    pub const INSTANCE_QUIT_WAIT: Duration = Duration::from_secs(5);
    /// All instance pipes together (SECURITY-7).
    pub const INSTANCES_TOTAL: Duration = Duration::from_secs(20);
    pub const MAX_INSTANCE_PIPES: usize = 16;
    pub const PROGRAMS_EXIT_WAIT: Duration = Duration::from_secs(30);
    pub const HELPERS_EXIT_WAIT: Duration = Duration::from_secs(75);
    /// D.8 step 4.
    pub const FILES_IN_USE_RETRY: Duration = Duration::from_secs(10);
    pub const INSTALLER_WAIT: Duration = Duration::from_secs(15 * 60);
    /// Total wait before H2 leaves `Run` behind (RELIABILITY-4).
    pub const INSTALLER_WAIT_MAX: Duration = Duration::from_secs(60 * 60);
    pub const RELAUNCH_WAIT: Duration = Duration::from_secs(10);
    /// The helper's `RecordTrust` lock wait.
    pub const TRUST_LOCK_WAIT: Duration = Duration::from_secs(2);
    /// Caller: after the last chunk, the longest wait for `HandedOff` (RELIABILITY-5: was 90 s).
    pub const HANDOFF_WAIT: Duration = Duration::from_secs(150);
    /// Caller: reading `Run` after losing the helper past the last chunk.
    pub const CALLER_RUN_POLL: Duration = Duration::from_secs(30);
    /// GUI: the hand-off overlay closes by itself after this long; [OK] closes it at once
    /// (OPS-UX-TEST-15; FIX-VERIFICATION-13 replaced the 5 s minimum). Must stay well below
    /// `CALLER_EXIT_WAIT` (tested: MAX + 2 s < CALLER_EXIT_WAIT).
    pub const HANDOFF_OVERLAY_MAX: Duration = Duration::from_secs(15);
    /// GUI: results older than this are marked seen silently (OPS-UX-TEST-6).
    pub const RESULT_SHOW_DAYS: u64 = 14;
    /// GUI: the "no successful check" / "expired" banner (SECURITY-11, OPS-UX-TEST-5).
    pub const STALE_NOTICE_DAYS: u64 = 30;
    /// GUI: NotFound becomes structural after this long.
    pub const NOT_FOUND_STRUCTURAL_DAYS: u64 = 7;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const RUN: &str = "0.2.1-3f9a0c2b7d1e4a65";

    #[test]
    fn run_ids() {
        let id = RunId::new(
            &Version::new(0, 2, 1),
            [0x3f, 0x9a, 0x0c, 0x2b, 0x7d, 0x1e, 0x4a, 0x65],
        );
        assert_eq!(id.as_str(), RUN);
        assert_eq!(RunId::parse(RUN), Ok(id.clone()));
        assert_eq!(id.version(), Version::new(0, 2, 1));
        assert_eq!(String::from(id.clone()), RUN);
        assert_eq!(RunId::try_from(RUN.to_string()), Ok(id));
        let edge = RunId::new(&Version::new(65_535, 0, 10), [0xff; 8]);
        assert_eq!(edge.as_str(), "65535.0.10-ffffffffffffffff");
        assert_eq!(RunId::parse(edge.as_str()), Ok(edge.clone()));
        assert_eq!(edge.version(), Version::new(65_535, 0, 10));
        assert_eq!(
            RunId::new(&Version::new(1, 0, 0), [0; 8]).as_str(),
            "1.0.0-0000000000000000"
        );
    }

    #[test]
    fn run_ids_have_one_spelling() {
        for text in [
            "",
            "0.2.1",
            "0.2.1-",
            "0.2-3f9a0c2b7d1e4a65",
            "0.2.1.4-3f9a0c2b7d1e4a65",
            "00.2.1-3f9a0c2b7d1e4a65",
            "0.02.1-3f9a0c2b7d1e4a65",
            "0.2.01-3f9a0c2b7d1e4a65",
            "65536.0.0-3f9a0c2b7d1e4a65",
            "0.99999.0-3f9a0c2b7d1e4a65",
            "0.100000.0-3f9a0c2b7d1e4a65",
            "v0.2.1-3f9a0c2b7d1e4a65",
            "0.2.1-3F9A0C2B7D1E4A65",
            "0.2.1-3f9a0c2b7d1e4a6",
            "0.2.1-3f9a0c2b7d1e4a655",
            "0.2.1-3f9a0c2b7d1e4a6g",
            "0.2.1--f9a0c2b7d1e4a65",
            "0.2.1-3f9a0c2b-d1e4a65",
            "0..1-3f9a0c2b7d1e4a65",
            ".2.1-3f9a0c2b7d1e4a65",
            "+0.2.1-3f9a0c2b7d1e4a65",
            " 0.2.1-3f9a0c2b7d1e4a65",
            "0.2.1-3f9a0c2b7d1e4a65 ",
            "0.2.1-3f9a0c2b7d1e4a65\\x",
            "0.2.١-3f9a0c2b7d1e4a65",
            "../0.2.1-3f9a0c2b7d1e4a65",
        ] {
            assert!(RunId::parse(text).is_err(), "{text:?}");
            assert!(
                serde_json::from_str::<RunId>(&serde_json::to_string(text).unwrap()).is_err(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn installer_exit_codes() {
        let table = [
            (0, InstallerExit::Success, false),
            (1, InstallerExit::UserCancelled, false),
            (2, InstallerExit::ScriptAborted, false),
            (20, InstallerExit::OsTooOld, true),
            (21, InstallerExit::WrongArch, true),
            (22, InstallerExit::HelperRunning, true),
            (23, InstallerExit::CliRunning, true),
            (24, InstallerExit::GuiRunning, true),
            (25, InstallerExit::Other(25), false),
            (26, InstallerExit::FilesInUse, true),
            (27, InstallerExit::FileWrite, true),
            (3, InstallerExit::Other(3), false),
            (19, InstallerExit::Other(19), false),
            (28, InstallerExit::Other(28), false),
            (3010, InstallerExit::Other(3010), false),
            (u32::MAX, InstallerExit::Other(u32::MAX), false),
        ];
        for (code, exit, leaves_old) in table {
            assert_eq!(classify_installer_exit(code), exit, "{code}");
            assert_eq!(exit.leaves_old_files(), leaves_old, "{code}");
        }
        assert_eq!(
            [
                nsis_exit::OS_TOO_OLD,
                nsis_exit::WRONG_ARCH,
                nsis_exit::HELPER_RUNNING,
                nsis_exit::CLI_RUNNING,
                nsis_exit::GUI_RUNNING,
                nsis_exit::FILES_IN_USE,
                nsis_exit::FILE_WRITE,
            ],
            [20, 21, 22, 23, 24, 26, 27]
        );
        assert_eq!(
            serde_json::to_string(&InstallerExit::Other(25)).unwrap(),
            r#"{"other":25}"#
        );
        assert_eq!(
            serde_json::to_string(&InstallerExit::FilesInUse).unwrap(),
            r#""files-in-use""#
        );
    }

    #[test]
    fn install_state_from_build_ids() {
        let state = InstallState::from_build_ids([
            Some("0.2.1+a".to_string()),
            None,
            Some("0.2.1+c".to_string()),
        ]);
        assert_eq!(state.gui.as_deref(), Some("0.2.1+a"));
        assert_eq!(state.cli, None);
        assert_eq!(state.helper.as_deref(), Some("0.2.1+c"));
    }

    /// The `Run` and `LastResult` values of design m5b H.5, through serde (the `to_json` /
    /// `from_json` wrappers are WP-H's).
    #[test]
    fn machine_record_shapes() {
        let run = r#"{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","phase":"installing","boot_id":"0b6d3c2a-9e1f-4d5a-8c7b-6a5f4e3d2c1b","started_at":1792022460000,"phase_at":1792022485000,"caller":{"pid":8532,"creation_time":134041234567890123},"caller_session":1,"stager":{"pid":9120,"creation_time":134041234600000000},"runner":{"pid":9344,"creation_time":134041234700000000},"installer":{"pid":9512,"creation_time":134041234800000000}}"#;
        let record: RunRecord = serde_json::from_str(run).unwrap();
        assert_eq!(record.phase, RunPhase::Installing);
        assert_eq!(record.run_id.as_str(), RUN);
        assert_eq!(serde_json::to_string(&record).unwrap(), run);
        assert!(
            serde_json::from_str::<RunRecord>(
                &run.replace("\"schema\":1,", "\"schema\":1,\"x\":0,")
            )
            .is_err()
        );

        let results = [
            r#"{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"installed"},"installer_exit":0,"installed_version":"0.2.1","gui_relaunch_attempted":true}"#,
            r#"{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"instance-busy","sessions":[2]}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}"#,
            r#"{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"files-in-use","programs":["gui"],"holders":[{"pid":7120,"session_id":2,"name":"powershell.exe"}]}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}"#,
            r#"{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"installer-refused","exit":"files-in-use"}},"installer_exit":26,"installed_version":"0.2.0","gui_relaunch_attempted":true}"#,
            r#"{"schema":1,"run_id":"0.2.1-3f9a0c2b7d1e4a65","from_version":"0.2.0","to_version":"0.2.1","arch":"x64","finished_at":1792022490000,"outcome":{"kind":"not-installed","detail":{"reason":"refused","code":"operation-open","waiting_for_reboot":true}},"installer_exit":null,"installed_version":"0.2.0","gui_relaunch_attempted":true}"#,
        ];
        let outcomes = [
            UpdateOutcome::Installed,
            UpdateOutcome::NotInstalled(NotInstalledReason::InstanceBusy { sessions: vec![2] }),
            UpdateOutcome::NotInstalled(NotInstalledReason::FilesInUse {
                programs: vec![ProgramKind::Gui],
                holders: vec![FileHolder {
                    pid: 7120,
                    session_id: 2,
                    name: "powershell.exe".to_string(),
                }],
            }),
            UpdateOutcome::NotInstalled(NotInstalledReason::InstallerRefused {
                exit: InstallerExit::FilesInUse,
            }),
            UpdateOutcome::NotInstalled(NotInstalledReason::Refused(
                UpdateRefusal::OperationOpen {
                    waiting_for_reboot: true,
                },
            )),
        ];
        for (json, outcome) in results.iter().zip(outcomes) {
            let result: UpdateResult = serde_json::from_str(json).unwrap();
            assert_eq!(result.outcome, outcome, "{json}");
            assert_eq!(&serde_json::to_string(&result).unwrap(), json);
        }

        let interrupted = interrupted_result(
            &record,
            Timestamp(1_792_022_490_000),
            &InstallState::default(),
        );
        assert_eq!(
            serde_json::to_value(&interrupted.outcome).unwrap(),
            serde_json::json!({"kind": "interrupted", "detail": {"phase": "installing"}})
        );
        // Lists that older records lack default to empty.
        let older: NotInstalledReason =
            serde_json::from_str(r#"{"reason":"programs-still-running","programs":["cli"]}"#)
                .unwrap();
        assert_eq!(
            older,
            NotInstalledReason::ProgramsStillRunning {
                programs: vec![ProgramKind::Cli],
                holders: Vec::new(),
            }
        );
    }

    #[test]
    fn waits_fit_together() {
        use timing::*;
        assert!(HANDOFF_OVERLAY_MAX + Duration::from_secs(2) < CALLER_EXIT_WAIT);
        assert!(READY_WAIT < HANDOFF_WAIT);
        assert!(INSTALLER_WAIT < INSTALLER_WAIT_MAX);
        assert!(INSTANCE_QUIT_WAIT < INSTANCES_TOTAL);
    }
}
