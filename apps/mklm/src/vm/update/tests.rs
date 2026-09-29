//! The update screens (design m5b F.4 "vm::update"): every state of the page (E.2), the banners
//! (E.3), the settings section, the hand-off and the results (D.13, E.5), and every row of the
//! table of E.6, in Japanese and English, compared with `tests/snapshots/update.{ja,en}.txt`
//! (`MKLM_BLESS=1` rewrites them; review the diff). The Japanese texts use only allowed Latin
//! words (E.7).

use std::path::PathBuf;

use mklm_client::update::cache::{CheckFailure, ClientState, ErrorClass, RollbackNote};
use mklm_client::update::check::{CheckError, CheckOutcome};
use mklm_client::update::download::DownloadError;
use mklm_client::update::env::Availability;
use mklm_client::update::stage::StageEnd;
use mklm_core::Timestamp;
use mklm_update::fetch::{FetchError, TransportError};
use mklm_update::run::{
    FailedReason, FileHolder, InstallerExit, NotInstalledReason, ProgramKind, RunId, RunPhase,
    UpdateOutcome, UpdateResult,
};
use mklm_update::{Freshness, OfferKind, UpdateRefusal, Version};

use super::*;
use crate::state::update::tests::{ISSUED, NOW, env, installed, offer, ready, verified};
use crate::state::update::{
    CheckPhase, DownloadPhase, ShownResult, UpdateNote, UpdateNotice, UpdateSessionEnd,
};
use crate::state::{AppState, OverlayKind, Page, SessionPhase, SessionPurpose, UpdateStage};

const DAY: u64 = 86_400;
const RUN: &str = "0.2.1-3f9a0c2b7d1e4a65";

/// Japan's time (UTC+9).
fn jst(unix: u64) -> LocalMinute {
    fixed_offset(540, unix)
}

fn dev_copy() -> Availability {
    Availability::NotInstalledCopy {
        exe_dir: PathBuf::from(r"D:\src\mklm\target\debug"),
        install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
    }
}

fn with_env(mut state: AppState, availability: Availability) -> AppState {
    state.update.env = Some(env(availability));
    state
}

fn checked(mut state: AppState, outcome: CheckOutcome) -> AppState {
    state.update.outcome = Some(outcome);
    state.update.client.last_check = Some(NOW);
    state.update.client.last_success = Some(NOW);
    state
}

fn failing(class: ErrorClass, id: &str, since: u64) -> ClientState {
    ClientState {
        last_check: Some(NOW),
        last_success: Some(since),
        last_failure: Some(CheckFailure {
            class,
            message_id: id.into(),
            first_at: since + 3600,
            at: NOW,
        }),
        ..ClientState::default()
    }
}

/// The page's scenes (design m5b E.2).
fn page_scenes() -> Vec<(&'static str, AppState)> {
    let mut scenes = vec![
        (
            "unavailable: no keys in this build",
            with_env(installed(), Availability::NotConfigured),
        ),
        (
            "unavailable: could not be read",
            with_env(
                installed(),
                Availability::Unknown {
                    detail: "the installation folder: SHGetKnownFolderPath failed".into(),
                },
            ),
        ),
        (
            "a development copy, not checked yet",
            with_env(installed(), dev_copy()),
        ),
        ("not checked yet", installed()),
    ];
    let mut checking = installed();
    checking.update.check = CheckPhase::Checking {
        token: 1,
        manual: true,
    };
    scenes.push(("checking", checking));
    scenes.push((
        "up to date",
        checked(
            installed(),
            CheckOutcome::UpToDate(verified("0.2.0", OfferKind::UpToDate)),
        ),
    ));
    scenes.push((
        "a new version, skipped, not downloaded",
        checked(installed(), CheckOutcome::Available(offer(false, true))),
    ));
    scenes.push((
        "a new version, not downloaded (automatic checks off)",
        checked(installed(), CheckOutcome::Available(offer(false, false))),
    ));
    let mut downloading = checked(installed(), CheckOutcome::Available(offer(false, false)));
    downloading.update.download = DownloadPhase::Downloading {
        token: 2,
        received: 2_831_155,
        total: 6_291_456,
    };
    scenes.push(("downloading", downloading));
    scenes.push(("ready, before the first prompt", ready()));
    let mut elevated = ready();
    elevated.elevated = true;
    scenes.push(("ready, an elevated MKLM", elevated));
    let mut open = ready();
    open.read.as_mut().unwrap().summary = mklm_client::startup::StartupSummary {
        items: vec![mklm_client::startup::AttentionItem {
            op: mklm_client::gate::OpRef {
                op_id: mklm_core::OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
                kind: mklm_core::OpKind::Migrate {
                    standard: mklm_core::Layout::Jis,
                    assignments: Vec::new(),
                },
                state: mklm_core::OpState::AwaitingConfirm,
            },
            attention: mklm_core::Attention::AwaitingUser,
        }],
        ..Default::default()
    };
    scenes.push(("ready, a change waits for the user", open));
    let mut arm = ready();
    arm.update.env.as_mut().unwrap().arm64_pc = true;
    scenes.push(("ready, an ARM64 PC running the x64 build", arm));
    scenes.push((
        "a manual update",
        checked(
            installed(),
            CheckOutcome::ManualRequired(verified("0.2.1", OfferKind::ManualRequired)),
        ),
    ));
    let mut asking = ready();
    asking.session = SessionPhase::Launching { id: 0 };
    asking.session_purpose = Some(SessionPurpose::Update);
    asking.page = Page::Update;
    scenes.push(("updating: the Windows prompt", asking));
    let mut sending = ready();
    sending.session = SessionPhase::Updating {
        id: 0,
        stage: UpdateStage::Sending {
            sent: 2_831_155,
            total: 6_291_456,
        },
    };
    sending.session_purpose = Some(SessionPurpose::Update);
    scenes.push(("updating: handing the installer over", sending));
    let mut starting = ready();
    starting.session = SessionPhase::Updating {
        id: 0,
        stage: UpdateStage::StartingRunner,
    };
    starting.session_purpose = Some(SessionPurpose::Update);
    scenes.push(("updating: Windows checks the files", starting));
    let mut disk = ready();
    disk.update.session_end = Some(UpdateSessionEnd::Stage(StageEnd::Refused(
        UpdateRefusal::DiskFull {
            needed: 30_000_000,
            available: 17_000_000,
        },
    )));
    scenes.push(("the update was refused: disk full", disk));
    let mut declined = ready();
    declined.update.note = Some(UpdateNote::Cancelled);
    scenes.push(("the Windows prompt was declined", declined));
    let mut mismatch = checked(installed(), CheckOutcome::Available(offer(false, false)));
    mismatch.update.download_error = Some(DownloadError::Fetch(FetchError::HashMismatch));
    scenes.push(("the download did not match", mismatch));
    let mut proxy = installed();
    proxy.update.check_error = Some(CheckError::Fetch(FetchError::Transport(
        TransportError::ProxyAuthRequired,
    )));
    scenes.push(("the check failed: a proxy that asks for credentials", proxy));
    let mut stale = installed();
    stale.update.check_error = Some(CheckError::Fetch(FetchError::Transport(
        TransportError::Timeout,
    )));
    stale.update.client = failing(ErrorClass::Transient, "upd-net", NOW - 31 * DAY);
    scenes.push(("the check failed for 31 days (transient)", stale));
    let mut expired = installed();
    let mut old = verified("0.2.0", OfferKind::UpToDate);
    old.freshness = Freshness::Expired;
    expired = checked(expired, CheckOutcome::UpToDate(old));
    expired.update.client.last_rollback = Some(RollbackNote {
        issued_at: ISSUED - 10 * DAY,
        seen: ISSUED,
        at: NOW,
    });
    expired.update.check_error = Some(CheckError::Refused(UpdateRefusal::Rollback {
        issued_at: ISSUED - 10 * DAY,
        seen: ISSUED,
    }));
    scenes.push((
        "up to date by old information: expired, and an older one ignored",
        expired,
    ));
    let mut out_of_step = ready();
    out_of_step.update.machine.consistent = None;
    scenes.push(("the installed programs are out of step", out_of_step));
    for (_, state) in &mut scenes {
        state.page = Page::Update;
    }
    scenes
}

/// The main screen's banners (design m5b E.3).
fn banner_scenes() -> Vec<(&'static str, AppState)> {
    let mut scenes = vec![("a new version is ready", ready())];
    for (name, notice) in [
        (
            "30 days without a check (transient)",
            UpdateNotice::Stale { structural: false },
        ),
        (
            "30 days without a check (structural)",
            UpdateNotice::Stale { structural: true },
        ),
        (
            "the information expired",
            UpdateNotice::Expired {
                expires: ISSUED + 180 * DAY,
            },
        ),
        (
            "an older manifest was ignored",
            UpdateNotice::Rollback {
                issued_at: ISSUED - DAY,
                seen: ISSUED,
            },
        ),
    ] {
        let mut state = installed();
        state.update.notice = Some(notice);
        scenes.push((name, state));
    }
    scenes
}

/// The settings section (design m5b E.2).
fn settings_scenes() -> Vec<(&'static str, AppState)> {
    let mut never = installed();
    never.update.check = CheckPhase::Idle;
    let up = checked(
        installed(),
        CheckOutcome::UpToDate(verified("0.2.0", OfferKind::UpToDate)),
    );
    let available = checked(installed(), CheckOutcome::Available(offer(true, false)));
    let mut stale = installed();
    stale.update.client = failing(ErrorClass::Structural, "upd-gh-changed", NOW - 40 * DAY);
    stale.update.check_error = Some(CheckError::Fetch(FetchError::TooManyRedirects));
    let mut off = up.clone();
    off.settings.update.auto_check = false;
    let mut old = verified("0.2.0", OfferKind::UpToDate);
    old.freshness = Freshness::Expired;
    let expired = checked(installed(), CheckOutcome::UpToDate(old));
    vec![
        ("never checked", never),
        ("up to date", up),
        ("a new version", available),
        ("failing for 40 days (structural)", stale),
        ("automatic checks off", off),
        ("expired", expired),
    ]
}

fn update_result(outcome: UpdateOutcome) -> UpdateResult {
    UpdateResult {
        schema: 1,
        run_id: RunId::parse(RUN).unwrap(),
        from_version: "0.2.0".into(),
        to_version: "0.2.1".into(),
        arch: mklm_update::Arch::X64,
        finished_at: Timestamp((NOW - 60) * 1000),
        installer_exit: None,
        installed_version: Some("0.2.0".into()),
        gui_relaunch_attempted: true,
        outcome,
    }
}

/// The results a start shows (design m5b D.13, E.5, E.6).
fn result_scenes() -> Vec<(&'static str, AppState)> {
    let shown = |result: UpdateResult, own: bool, closed: bool| {
        let mut state = installed();
        if result.outcome == UpdateOutcome::Installed
            || result.installed_version.as_deref() == Some("0.2.1")
        {
            state.update.env.as_mut().unwrap().installed = Version::new(0, 2, 1);
            state.update.machine.consistent = Some(Version::new(0, 2, 1));
        }
        state.update.shown = Some(ShownResult::Result {
            result: Box::new(result),
            own,
            closed_for_update: closed,
        });
        state.overlay = OverlayKind::UpdateResult;
        state
    };
    let installed_result = UpdateResult {
        installer_exit: Some(0),
        installed_version: Some("0.2.1".into()),
        ..update_result(UpdateOutcome::Installed)
    };
    let not_installed = |reason| update_result(UpdateOutcome::NotInstalled(reason));
    let holders = vec![FileHolder {
        pid: 7120,
        session_id: 2,
        name: "powershell.exe".into(),
    }];
    let mut out_of_step = ready();
    out_of_step.update.machine.consistent = None;
    out_of_step.update.shown = Some(ShownResult::Inconsistent {
        version: Version::new(0, 2, 1),
        cached: true,
    });
    out_of_step.overlay = OverlayKind::UpdateResult;
    let interrupted = |phase, installed_version: &str| UpdateResult {
        installed_version: Some(installed_version.into()),
        ..update_result(UpdateOutcome::Interrupted { phase })
    };
    vec![
        (
            "installed (this user's)",
            shown(installed_result.clone(), true, false),
        ),
        (
            "installed by another user, this MKLM had quit for it",
            shown(installed_result, false, true),
        ),
        (
            "another user's MKLM was busy",
            shown(
                not_installed(NotInstalledReason::InstanceBusy { sessions: vec![2] }),
                true,
                false,
            ),
        ),
        (
            "MKLM programs still ran",
            shown(
                not_installed(NotInstalledReason::ProgramsStillRunning {
                    programs: vec![ProgramKind::Gui, ProgramKind::Cli],
                    holders: holders.clone(),
                }),
                true,
                false,
            ),
        ),
        (
            "the files were in use",
            shown(
                not_installed(NotInstalledReason::FilesInUse {
                    programs: vec![ProgramKind::Gui],
                    holders,
                }),
                true,
                false,
            ),
        ),
        (
            "the installer refused: files in use",
            shown(
                UpdateResult {
                    installer_exit: Some(26),
                    ..not_installed(NotInstalledReason::InstallerRefused {
                        exit: InstallerExit::FilesInUse,
                    })
                },
                true,
                false,
            ),
        ),
        (
            "the installer refused: Windows too old",
            shown(
                not_installed(NotInstalledReason::InstallerRefused {
                    exit: InstallerExit::OsTooOld,
                }),
                true,
                false,
            ),
        ),
        (
            "refused again by the runner: a restart is pending",
            shown(
                not_installed(NotInstalledReason::Refused(UpdateRefusal::OperationOpen {
                    waiting_for_reboot: true,
                })),
                true,
                false,
            ),
        ),
        (
            "an antivirus stopped the installer",
            shown(
                not_installed(NotInstalledReason::InstallerNotStarted { code: 225 }),
                true,
                false,
            ),
        ),
        (
            "the installer timed out",
            shown(
                update_result(UpdateOutcome::Failed(FailedReason::InstallerTimedOut)),
                true,
                false,
            ),
        ),
        (
            "interrupted before anything was replaced",
            shown(interrupted(RunPhase::Waiting, "0.2.0"), true, false),
        ),
        (
            "interrupted, the new version in place",
            shown(interrupted(RunPhase::Finishing, "0.2.1"), true, false),
        ),
        (
            "interrupted while installing, the old version kept",
            shown(interrupted(RunPhase::Installing, "0.2.0"), true, false),
        ),
        ("the installed programs are out of step", out_of_step),
    ]
}

fn table(lang: Lang) -> String {
    let params = Params {
        installed: "0.2.0".into(),
        offered: "0.2.1".into(),
        path: r"D:\src\mklm\target\debug".into(),
        installer: "MKLM-Setup-0.2.1-x64.exe".into(),
        needed_mb: 13,
        programs: text::list(&["MKLM", "mklm-cli"], lang),
        reason: text::installer_env_reason(true, lang),
        date: date(ISSUED + 180 * DAY, &jst, lang),
        state_now: text::version_now("0.2.0", lang),
    };
    Msg::ALL
        .iter()
        .map(|msg| format!("{}: {}\n", msg.id(), words(*msg, &params, lang)))
        .collect()
}

fn all(lang: Lang) -> String {
    let mut out = String::new();
    for (name, state) in page_scenes() {
        out.push_str(&format!(
            "== page: {name} ==\n{}\n",
            update_page(&state, &jst, lang).snapshot_text()
        ));
    }
    for (name, state) in banner_scenes() {
        let banner = banner(&state, lang).unwrap();
        out.push_str(&format!(
            "== banner: {name} ==\n{}\n",
            banner.snapshot_text()
        ));
    }
    for (name, state) in settings_scenes() {
        out.push_str(&format!(
            "== settings: {name} ==\n{}\n",
            settings_section(&state, &jst, lang).snapshot_text()
        ));
    }
    let (title, message) = handoff(lang);
    out.push_str(&format!(
        "== hand-off ==\ntitle: {title}\nmessage: {message}\n\n"
    ));
    for (name, state) in result_scenes() {
        let result = result_overlay(&state, lang).unwrap();
        out.push_str(&format!(
            "== result: {name} ==\n{}\n",
            result.snapshot_text()
        ));
    }
    let mut uac = ready();
    uac.uac_origin = crate::state::UacNoticeOrigin::Update;
    out.push_str(&format!(
        "== the UAC explanation before an update ==\nline: {}\n\n",
        uac_location_line(&uac, lang)
    ));
    out.push_str("== the messages of design m5b E.6 ==\n");
    out.push_str(&table(lang));
    out
}

/// `tests/snapshots/<name>` compared line by line, or written with `MKLM_BLESS=1` (as
/// tests/common/mod.rs does for the integration tests).
fn assert_snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(name);
    if std::env::var_os("MKLM_BLESS").is_some_and(|value| value == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| {
            panic!(
                "{}: {error}; run with MKLM_BLESS=1 to create it",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    if expected == actual {
        return;
    }
    for (line, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        assert_eq!(
            want,
            got,
            "{} differs at line {} (MKLM_BLESS=1 rewrites the file; review the diff)",
            path.display(),
            line + 1
        );
    }
    panic!(
        "{} differs in length (MKLM_BLESS=1 rewrites it)",
        path.display()
    );
}

#[test]
fn the_update_screens_in_japanese() {
    assert_snapshot("update.ja.txt", &all(Lang::Ja));
}

#[test]
fn the_update_screens_in_english() {
    assert_snapshot("update.en.txt", &all(Lang::En));
}

/// Japanese update texts: only allowed Latin words, and the names the texts carry (E.7).
#[test]
fn japanese_update_texts_use_only_allowed_latin_words() {
    let names = [
        "MKLM-Setup-0.2.1-x64.exe",
        r"C:\Program Files\SHIN DATA CENTER\MKLM\mklm-helper.exe",
        r"D:\src\mklm\target\debug",
        "powershell.exe",
    ];
    let lang = Lang::Ja;
    let mut texts: Vec<String> = Vec::new();
    let buttons = |texts: &mut Vec<String>, actions: &[ActionView]| {
        for action in actions {
            texts.push(action.text.clone());
            texts.push(action.label.clone());
        }
    };
    for (_, state) in page_scenes() {
        let page = update_page(&state, &jst, lang);
        texts.extend([
            page.title,
            page.status,
            page.version_line,
            page.published,
            page.progress_text,
            page.progress_label,
            page.progress_announcement,
            page.explanation,
        ]);
        texts.extend(page.info);
        buttons(&mut texts, &page.buttons);
        buttons(&mut texts, &page.links);
    }
    for (_, state) in banner_scenes() {
        let banner = banner(&state, lang).unwrap();
        texts.push(banner.text);
        buttons(&mut texts, &banner.actions);
    }
    for (_, state) in settings_scenes() {
        let section = settings_section(&state, &jst, lang);
        texts.extend([
            section.last_check,
            section.last_success,
            section.stale,
            section.expired,
        ]);
    }
    let (title, message) = handoff(lang);
    texts.extend([title, message]);
    for (_, state) in result_scenes() {
        let result = result_overlay(&state, lang).unwrap();
        texts.extend([result.title, result.message]);
        buttons(&mut texts, &result.actions);
    }
    let mut uac = ready();
    uac.uac_origin = crate::state::UacNoticeOrigin::Update;
    texts.push(uac_location_line(&uac, lang));
    texts.extend(
        table(lang)
            .lines()
            .filter_map(|line| line.split_once(": ").map(|(_, value)| value.to_string())),
    );
    texts.push(text::tray_tooltip("0.2.1", lang));
    texts.push(text::wizard_daily_check(lang));
    for text in texts.iter().filter(|text| !text.is_empty()) {
        let unexpected = crate::vm::unexpected_latin(text, &names);
        assert!(unexpected.is_empty(), "{text}: {unexpected:?}");
    }
}

/// Every `UpdateRefusal` (design m5b H.1).
fn every_refusal() -> Vec<UpdateRefusal> {
    let text = || "x".to_string();
    vec![
        UpdateRefusal::NotConfigured,
        UpdateRefusal::NotInstalledCopy,
        UpdateRefusal::ManifestTooLarge { len: 1 },
        UpdateRefusal::SignatureTooLarge { len: 1 },
        UpdateRefusal::SignatureMalformed,
        UpdateRefusal::WrongTrustedComment,
        UpdateRefusal::UnknownKey { key_id: text() },
        UpdateRefusal::BadSignature,
        UpdateRefusal::RevokedKey { key_id: text() },
        UpdateRefusal::ManifestMalformed { detail: text() },
        UpdateRefusal::UnsupportedSchema { schema: 2 },
        UpdateRefusal::WrongProduct,
        UpdateRefusal::WrongChannel,
        UpdateRefusal::SignerNotListed { key_id: text() },
        UpdateRefusal::IllegalRevocation { key_id: text() },
        UpdateRefusal::BadVersion { text: text() },
        UpdateRefusal::TagMismatch {
            tag: "v0.2.2".into(),
            version: "0.2.1".into(),
        },
        UpdateRefusal::BadTimestamps,
        UpdateRefusal::Rollback {
            issued_at: 1,
            seen: 2,
        },
        UpdateRefusal::NoAssetForArch {
            arch: mklm_update::Arch::Arm64,
        },
        UpdateRefusal::AssetMalformed { detail: text() },
        UpdateRefusal::NotNewer {
            offered: "0.2.1".into(),
            installed: "0.2.1".into(),
        },
        UpdateRefusal::ManualUpdateRequired {
            min_from: "0.2.0".into(),
            installed: "0.1.0".into(),
        },
        UpdateRefusal::Busy,
        UpdateRefusal::UpdateInProgress,
        UpdateRefusal::OperationOpen {
            waiting_for_reboot: false,
        },
        UpdateRefusal::OperationOpen {
            waiting_for_reboot: true,
        },
        UpdateRefusal::RecoveryNeeded,
        UpdateRefusal::JournalUnreadable,
        UpdateRefusal::DiskFull {
            needed: 1 << 30,
            available: 1 << 20,
        },
        UpdateRefusal::InstallerSizeMismatch {
            expected: 2,
            received: 1,
        },
        UpdateRefusal::InstallerHashMismatch,
        UpdateRefusal::ChunkMalformed,
        UpdateRefusal::ChunkOutOfOrder {
            expected: 0,
            found: 1,
        },
        UpdateRefusal::CallerLeft,
        UpdateRefusal::HandOffFailed { detail: text() },
        UpdateRefusal::Storage { detail: text() },
        UpdateRefusal::Internal { detail: text() },
    ]
}

/// Design m5b E.6: every `NotInstalled` and `Failed` result says the keyboard settings did not
/// change, whatever message it maps to — a refusal of the runner too (D.7 steps 8–10), which
/// also says that MKLM stays at its version.
#[test]
fn every_not_installed_or_failed_result_says_the_keyboards_did_not_change() {
    let mut outcomes: Vec<UpdateOutcome> = every_refusal()
        .into_iter()
        .map(|refusal| UpdateOutcome::NotInstalled(NotInstalledReason::Refused(refusal)))
        .collect();
    let reasons = [
        NotInstalledReason::CallerDidNotExit,
        NotInstalledReason::InstanceBusy { sessions: vec![2] },
        NotInstalledReason::ProgramsStillRunning {
            programs: vec![ProgramKind::Cli],
            holders: Vec::new(),
        },
        NotInstalledReason::FilesInUse {
            programs: Vec::new(),
            holders: Vec::new(),
        },
        NotInstalledReason::DiskFull {
            needed: 1 << 30,
            available: 1 << 20,
        },
        NotInstalledReason::SessionEnding,
        NotInstalledReason::InstalledVersionChanged {
            found: Some("0.2.0".into()),
        },
        NotInstalledReason::InstallerNotStarted { code: 225 },
        NotInstalledReason::InstallerNotStarted { code: 5 },
        NotInstalledReason::InstallerExit { code: 1 },
    ];
    outcomes.extend(reasons.into_iter().map(UpdateOutcome::NotInstalled));
    for exit in [
        InstallerExit::Success,
        InstallerExit::UserCancelled,
        InstallerExit::ScriptAborted,
        InstallerExit::HelperRunning,
        InstallerExit::CliRunning,
        InstallerExit::GuiRunning,
        InstallerExit::FilesInUse,
        InstallerExit::OsTooOld,
        InstallerExit::WrongArch,
        InstallerExit::FileWrite,
        InstallerExit::Other(99),
    ] {
        outcomes.push(UpdateOutcome::NotInstalled(
            NotInstalledReason::InstallerRefused { exit },
        ));
    }
    outcomes.extend(
        [
            FailedReason::InstallerTimedOut,
            FailedReason::Inconsistent,
            FailedReason::UnexpectedVersion {
                found: "0.1.9".into(),
            },
        ]
        .map(UpdateOutcome::Failed),
    );
    for outcome in outcomes {
        let refused = matches!(
            outcome,
            UpdateOutcome::NotInstalled(NotInstalledReason::Refused(_))
        );
        for (own, closed) in [(true, false), (false, true)] {
            for inconsistent in [false, true] {
                let mut state = installed();
                if inconsistent {
                    state.update.machine.consistent = None;
                }
                state.update.shown = Some(ShownResult::Result {
                    result: Box::new(update_result(outcome.clone())),
                    own,
                    closed_for_update: closed,
                });
                for lang in [Lang::Ja, Lang::En] {
                    let message = result_overlay(&state, lang).unwrap().message;
                    let keyboards = match lang {
                        Lang::Ja => "キーボードの設定",
                        Lang::En => "keyboard settings",
                    };
                    assert!(
                        message.contains(keyboards),
                        "{outcome:?} {lang:?}: {message}"
                    );
                    if refused {
                        assert!(message.contains("0.2.0"), "{outcome:?} {lang:?}: {message}");
                    }
                }
            }
        }
    }
}

/// The messages of the check's failures are the ones the user's record keeps (E.6; the class and
/// ID of `mklm_client::update::classify`).
#[test]
fn check_failures_agree_with_the_record() {
    let errors = [
        CheckError::Fetch(FetchError::Transport(TransportError::Timeout)),
        CheckError::Fetch(FetchError::Transport(TransportError::Tls)),
        CheckError::Fetch(FetchError::Transport(TransportError::ProxyAuthRequired)),
        CheckError::Fetch(FetchError::NotFound),
        CheckError::Fetch(FetchError::RateLimited { status: 429 }),
        CheckError::Fetch(FetchError::HttpStatus { status: 500 }),
        CheckError::Fetch(FetchError::TooManyRedirects),
        CheckError::Fetch(FetchError::RedirectNotAllowed {
            location: "x".into(),
        }),
        CheckError::Fetch(FetchError::MissingLocation),
        CheckError::Fetch(FetchError::UnexpectedEncoding),
        CheckError::Fetch(FetchError::TooLarge { limit: 1 }),
        CheckError::Fetch(FetchError::DeadlineExceeded),
        CheckError::Fetch(FetchError::Sink { detail: "x".into() }),
        CheckError::Refused(UpdateRefusal::BadSignature),
        CheckError::Refused(UpdateRefusal::UnknownKey { key_id: "x".into() }),
        CheckError::Refused(UpdateRefusal::ManifestMalformed { detail: "x".into() }),
        CheckError::Refused(UpdateRefusal::TagMismatch {
            tag: "v0.2.2".into(),
            version: "0.2.1".into(),
        }),
        CheckError::Refused(UpdateRefusal::Rollback {
            issued_at: 1,
            seen: 2,
        }),
        CheckError::Cache("x".into()),
        CheckError::Unavailable(Availability::Unknown { detail: "x".into() }),
    ];
    for error in errors {
        for days in [0, 8] {
            let (_, id) = mklm_client::update::classify::classify(&error, days).unwrap();
            assert_eq!(explain_check(&error, days).msg.id(), id, "{error:?} {days}");
        }
    }
    // Not failures: the cancelled text, and the unavailable reasons.
    assert_eq!(
        explain_check(&CheckError::Fetch(FetchError::Cancelled), 0).msg,
        Msg::Cancelled
    );
    assert_eq!(
        explain_check(&CheckError::Unavailable(Availability::NotConfigured), 0).msg,
        Msg::NotConfigured
    );
}

/// The next steps of design m5b E.6's table.
#[test]
fn next_steps() {
    use UpdateAction::*;
    let steps = |explained: Explained| explained.actions;
    assert_eq!(
        steps(explain_refusal(&UpdateRefusal::OperationOpen {
            waiting_for_reboot: false
        })),
        vec![Review]
    );
    assert_eq!(
        steps(explain_refusal(&UpdateRefusal::RecoveryNeeded)),
        vec![Review]
    );
    assert_eq!(
        steps(explain_refusal(&UpdateRefusal::OperationOpen {
            waiting_for_reboot: true
        })),
        vec![Restart]
    );
    assert_eq!(
        steps(explain_refusal(&UpdateRefusal::Busy)),
        vec![UpdateNow]
    );
    assert_eq!(
        steps(explain_refusal(&UpdateRefusal::InstallerHashMismatch)),
        vec![Download]
    );
    assert_eq!(
        steps(explain_refusal(&UpdateRefusal::Internal {
            detail: "x".into()
        })),
        vec![CopyDetails]
    );
    assert_eq!(
        steps(explain_not_installed(&NotInstalledReason::FilesInUse {
            programs: Vec::new(),
            holders: Vec::new()
        })),
        vec![UpdateNow, CopyDetails]
    );
    assert_eq!(
        explain_not_installed(&NotInstalledReason::InstallerNotStarted { code: 226 }).msg,
        Msg::AvBlocked
    );
    assert_eq!(
        explain_not_installed(&NotInstalledReason::InstallerNotStarted { code: 5 }).msg,
        Msg::InstallerNotStarted
    );
    for exit in [
        InstallerExit::HelperRunning,
        InstallerExit::CliRunning,
        InstallerExit::GuiRunning,
    ] {
        assert_eq!(explain_installer_exit(exit).msg, Msg::ProgramsRunning);
    }
    for exit in [
        InstallerExit::Success,
        InstallerExit::UserCancelled,
        InstallerExit::ScriptAborted,
        InstallerExit::Other(25),
    ] {
        assert_eq!(explain_installer_exit(exit).msg, Msg::NotInstalled);
    }
    assert_eq!(
        steps(explain_failed(&FailedReason::UnexpectedVersion {
            found: "0.1.0".into()
        })),
        vec![RunInstaller, ReleasePage, CopyDetails]
    );
    assert_eq!(
        explain_stage(&StageEnd::SourceChanged).msg,
        Msg::DownloadMismatch
    );
    assert_eq!(
        explain_stage(&StageEnd::Unresponsive).msg,
        Msg::PrepareFailed
    );
    assert_eq!(
        explain_session_end(&UpdateSessionEnd::NotLaunched(
            mklm_client::orchestrator::LaunchError::Declined
        ))
        .msg,
        Msg::Cancelled
    );
    assert_eq!(explain_freshness(Freshness::Fresh), None);
    assert_eq!(
        explain_freshness(Freshness::Expired).map(|e| e.msg),
        Some(Msg::Expired)
    );
    assert_eq!(
        explain_fetch(&FetchError::HashMismatch, true, 0).actions,
        vec![Download]
    );
    assert_eq!(
        explain_fetch(&FetchError::DeadlineExceeded, true, 0).msg,
        Msg::NetDl
    );
}

#[test]
fn actions_round_trip_through_their_index() {
    for action in UpdateAction::ALL {
        assert_eq!(UpdateAction::from_index(action.index()), Some(action));
    }
    assert_eq!(UpdateAction::from_index(-1), None);
    assert_eq!(UpdateAction::from_index(99), None);
}

#[test]
fn dates_in_the_machine_s_zone() {
    assert_eq!(date(ISSUED, &jst, Lang::Ja), "2026/10/15");
    assert_eq!(date(ISSUED, &jst, Lang::En), "2026-10-15");
    assert_eq!(date_time(NOW, &jst, Lang::Ja), "2026/10/15 10:00");
    assert_eq!(
        fixed_offset(-300, 0),
        LocalMinute {
            year: 1969,
            month: 12,
            day: 31,
            hour: 19,
            minute: 0
        }
    );
}

/// The tray names a ready update; a skipped one it does not (E.3).
#[test]
fn the_tray() {
    let (tooltip, can) = tray(&ready(), Lang::Ja);
    assert_eq!(
        tooltip.as_deref(),
        Some("MKLM — 新しい版（0.2.1）があります")
    );
    assert!(can);
    let mut skipped = ready();
    skipped.update.outcome = Some(CheckOutcome::Available(offer(true, true)));
    assert_eq!(tray(&skipped, Lang::Ja).0, None);
    assert_eq!(tray(&installed(), Lang::Ja), (None, false));
}

/// The page's buttons are never "verified" for the run-installer path (RED-TEAM-2), and every
/// button has an accessible name (E.7).
#[test]
fn every_button_has_a_name() {
    for (name, state) in page_scenes() {
        for lang in [Lang::Ja, Lang::En] {
            let page = update_page(&state, &jst, lang);
            for button in page.buttons.iter().chain(&page.links) {
                assert!(!button.label.is_empty(), "{name}: {button:?}");
                assert!(!button.text.is_empty(), "{name}: {button:?}");
            }
        }
    }
    let run = action(UpdateAction::RunInstaller, &Version::new(0, 2, 1), Lang::Ja);
    assert_eq!(run.label, "インストーラーを実行（管理者の許可が要ります）");
}
