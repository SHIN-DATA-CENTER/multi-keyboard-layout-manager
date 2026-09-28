//! The update rules of the GUI (design m5b F.4 "state::tests").

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mklm_client::gate::OpRef;
use mklm_client::session::{Notice, Prompt, SessionKind, SessionView};
use mklm_client::startup::{AttentionItem, StartupSummary};
use mklm_client::update::cache::{CheckFailure, ClientState, ErrorClass, RollbackNote};
use mklm_client::update::check::{CheckError, CheckOutcome, Offer};
use mklm_client::update::env::Availability;
use mklm_client::update::stage::StageEnd;
use mklm_core::{Attention, Layout, OpId, OpKind, OpState, Timestamp};
use mklm_update::fetch::FetchError;
use mklm_update::run::timing::{CALLER_EXIT_WAIT, HANDOFF_OVERLAY_MAX, RESULT_SHOW_DAYS};
use mklm_update::run::{
    FailedReason, NotInstalledReason, RunId, RunPhase, RunView, UpdateOutcome, UpdateResult,
};
use mklm_update::{
    Arch, Freshness, KeyFingerprint, KeyId, KeyRole, OfferKind, SelectedAsset, Sha256Digest,
    SignatureSlot, UpdateRefusal, VerifiedManifest, Version,
};

use super::*;
use crate::state::{AppMsg, SystemRead};

pub(crate) const NOW: u64 = 1_792_026_000;
pub(crate) const ISSUED: u64 = 1_792_022_400;
const DAY: u64 = 86_400;
const RUN: &str = "0.2.1-3f9a0c2b7d1e4a65";

pub(crate) fn verified(version: &str, offer: OfferKind) -> VerifiedManifest {
    let version = Version::parse(version).unwrap();
    VerifiedManifest {
        asset: SelectedAsset {
            arch: Arch::X64,
            name: mklm_update::installer_name(&version, Arch::X64),
            size: 6_291_456,
            sha256: Sha256Digest([3; 32]),
        },
        version,
        issued_at: ISSUED,
        expires: ISSUED + 180 * DAY,
        signer: KeyId([1; 8]),
        signer_role: KeyRole::Primary,
        signer_fingerprint: KeyFingerprint([2; 32]),
        signer_is_dev: false,
        key_ids: vec![KeyId([1; 8])],
        revoked: BTreeMap::new(),
        min_from_version: None,
        freshness: Freshness::Fresh,
        offer,
    }
}

pub(crate) fn offer(downloaded: bool, skipped: bool) -> Offer {
    Offer {
        verified: verified("0.2.1", OfferKind::Newer),
        manifest: b"{}".to_vec(),
        signature: b"sig".to_vec(),
        slot: SignatureSlot::Main,
        skipped,
        downloaded: downloaded.then(|| {
            PathBuf::from(
                r"C:\Users\u\AppData\Local\SHIN DATA CENTER\MKLM\update\MKLM-Setup-0.2.1-x64.exe",
            )
        }),
    }
}

pub(crate) fn env(availability: Availability) -> EnvView {
    EnvView {
        installed: Version::new(0, 2, 0),
        availability,
        install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
        arm64_pc: false,
    }
}

/// An installed MKLM 0.2.0, in step, nothing checked yet, the journal read and quiet.
pub(crate) fn installed() -> AppState {
    let mut state = AppState {
        now_unix: Some(NOW),
        visible: true,
        read: Some(SystemRead {
            snapshot: None,
            warnings: Vec::new(),
            journal: None,
            boot: None,
            summary: StartupSummary::default(),
        }),
        ..AppState::default()
    };
    state.update.env = Some(env(Availability::Available));
    state.update.machine.consistent = Some(Version::new(0, 2, 0));
    state.update.machine.install_known = true;
    state
}

/// [`installed`] with a verified download of 0.2.1 ready.
pub(crate) fn ready() -> AppState {
    let mut state = installed();
    state.update.outcome = Some(CheckOutcome::Available(offer(true, false)));
    state.settings.change.uac_notice_seen = true;
    state
}

fn start(state: &mut AppState, machine: MachineView, client: ClientState) -> Vec<Effect> {
    let env = state.update.env.clone().unwrap();
    handle(
        state,
        UpdateMsg::Started(Box::new(UpdateStart {
            env,
            machine,
            client,
            cached: None,
        })),
    )
}

fn schedules(effects: &[Effect]) -> Vec<(Duration, Duration)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ScheduleUpdateCheck { after, jitter } => Some((*after, *jitter)),
            _ => None,
        })
        .collect()
}

fn check_task(effects: &[Effect]) -> Option<(u64, bool)> {
    effects.iter().find_map(|effect| match effect {
        Effect::Update(UpdateTask::Check { token, manual, .. }) => Some((*token, *manual)),
        _ => None,
    })
}

fn has_download(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Update(UpdateTask::Download { .. })))
}

fn saves(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::SaveSettings(_)))
}

// --- The schedule (design m5b E.1; RELIABILITY-8) --------------------------------------------

#[test]
fn the_first_check_after_a_start() {
    let client = |last_check: Option<u64>, last_success: Option<u64>| ClientState {
        last_check,
        last_success,
        ..ClientState::default()
    };
    let soon = (Duration::from_secs(60), Duration::from_secs(120));
    // Never checked, or 24 hours ago or more: 60 s + up to 120 s.
    assert_eq!(first_schedule(&client(None, None), NOW), soon);
    assert_eq!(
        first_schedule(&client(Some(NOW - DAY), Some(NOW - DAY)), NOW),
        soon
    );
    // Two hours ago: 24 hours after it + up to 60 minutes.
    assert_eq!(
        first_schedule(&client(Some(NOW - 7200), Some(NOW - 7200)), NOW),
        (Duration::from_secs(DAY - 7200), Duration::from_secs(3600))
    );
    // A clock that ran ahead: a check or a success more than an hour in the future counts as
    // never checked (RELIABILITY-8).
    assert_eq!(first_schedule(&client(Some(NOW + 7200), None), NOW), soon);
    assert_eq!(
        first_schedule(&client(Some(NOW - 60), Some(NOW + 7200)), NOW),
        soon
    );
    // Less than an hour ahead is a clock's small difference.
    assert_eq!(
        first_schedule(&client(Some(NOW + 1800), None), NOW),
        (Duration::from_secs(DAY + 1800), Duration::from_secs(3600))
    );
}

#[test]
fn checks_follow_the_setting_and_the_day() {
    let mut state = installed();
    let effects = start(&mut state, MachineView::default(), ClientState::default());
    assert_eq!(
        schedules(&effects),
        vec![(Duration::from_secs(60), Duration::from_secs(120))]
    );
    assert_eq!(state.update.next_check_at, Some(NOW + 60));
    // The timer: a check, then the next one 24 h + up to 60 min after it.
    let effects = handle(&mut state, UpdateMsg::CheckDue);
    let (token, manual) = check_task(&effects).unwrap();
    assert!(!manual);
    let effects = handle(
        &mut state,
        UpdateMsg::Checked {
            token,
            result: Box::new(Ok(CheckOutcome::UpToDate(verified(
                "0.2.0",
                OfferKind::UpToDate,
            )))),
            client: Box::new(ClientState {
                last_check: Some(NOW),
                last_success: Some(NOW),
                ..ClientState::default()
            }),
        },
    );
    assert_eq!(
        schedules(&effects),
        vec![(Duration::from_secs(DAY), Duration::from_secs(3600))]
    );
    // Off: no automatic check at all, "今すぐ確認" still works.
    let effects = handle(&mut state, UpdateMsg::AutoCheck(false));
    assert!(effects.contains(&Effect::StopUpdateCheck));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SaveSettings(s) if !s.update.auto_check))
    );
    assert!(check_task(&handle(&mut state, UpdateMsg::CheckDue)).is_none());
    let effects = handle(&mut state, UpdateMsg::CheckNow);
    assert_eq!(check_task(&effects).map(|(_, manual)| manual), Some(true));
    // Off at start: nothing scheduled.
    let mut off = installed();
    off.settings.update.auto_check = false;
    let effects = start(&mut off, MachineView::default(), ClientState::default());
    assert!(schedules(&effects).is_empty());
    assert!(effects.contains(&Effect::StopUpdateCheck));
}

#[test]
fn a_resume_past_the_schedule_checks_30_seconds_later() {
    let mut state = installed();
    start(&mut state, MachineView::default(), ClientState::default());
    let due = state.update.next_check_at.unwrap();
    state.now_unix = Some(due - 10);
    assert!(handle(&mut state, UpdateMsg::Resumed).is_empty());
    state.now_unix = Some(due + 3600);
    assert_eq!(
        schedules(&handle(&mut state, UpdateMsg::Resumed)),
        vec![(Duration::from_secs(30), Duration::ZERO)]
    );
}

#[test]
fn a_development_copy_checks_and_an_unconfigured_build_does_not() {
    let mut copy = installed();
    copy.update.env = Some(env(Availability::NotInstalledCopy {
        exe_dir: PathBuf::from(r"D:\src\mklm\target\debug"),
        install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
    }));
    assert!(check_task(&handle(&mut copy, UpdateMsg::CheckNow)).is_some());
    let mut keyless = installed();
    keyless.update.env = Some(env(Availability::NotConfigured));
    assert!(handle(&mut keyless, UpdateMsg::CheckNow).is_empty());
    let effects = start(&mut keyless, MachineView::default(), ClientState::default());
    assert!(schedules(&effects).is_empty());
}

// --- The automatic download (design m5b E.1; C.5) --------------------------------------------

fn checked_offer(state: &mut AppState, offer: Offer) -> Vec<Effect> {
    let (token, _) = check_task(&handle(state, UpdateMsg::CheckNow)).unwrap();
    handle(
        state,
        UpdateMsg::Checked {
            token,
            result: Box::new(Ok(CheckOutcome::Available(offer))),
            client: Box::new(ClientState::default()),
        },
    )
}

#[test]
fn a_new_version_downloads_by_itself_unless_skipped_or_off() {
    let mut state = installed();
    assert!(has_download(&checked_offer(
        &mut state,
        offer(false, false)
    )));
    assert!(matches!(
        state.update.download,
        DownloadPhase::Downloading {
            total: 6_291_456,
            ..
        }
    ));
    // Skipped: only the page's "ダウンロード".
    let mut skipped = installed();
    assert!(!has_download(&checked_offer(
        &mut skipped,
        offer(false, true)
    )));
    assert_eq!(page_state(&skipped), PageState::Skipped);
    assert!(has_download(&handle(&mut skipped, UpdateMsg::Download)));
    // Automatic checks off: nothing goes to the network by itself.
    let mut off = installed();
    off.settings.update.auto_check = false;
    assert!(!has_download(&checked_offer(&mut off, offer(false, false))));
    assert_eq!(page_state(&off), PageState::NotDownloaded);
    // Already downloaded: ready.
    let mut done = installed();
    assert!(!has_download(&checked_offer(&mut done, offer(true, false))));
    assert_eq!(page_state(&done), PageState::Ready);
    assert_eq!(banner(&done), Some(Banner::Available));
    // A development copy never downloads.
    let mut copy = installed();
    copy.update.env = Some(env(Availability::NotInstalledCopy {
        exe_dir: PathBuf::from(r"D:\dev"),
        install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
    }));
    assert!(!has_download(&checked_offer(
        &mut copy,
        offer(false, false)
    )));
}

#[test]
fn the_download_s_answers() {
    let mut state = installed();
    checked_offer(&mut state, offer(false, false));
    let DownloadPhase::Downloading { token, .. } = state.update.download else {
        panic!("not downloading");
    };
    // Progress renders on a new percent only.
    assert_eq!(
        handle(
            &mut state,
            UpdateMsg::DownloadProgress {
                token,
                received: 65_536,
                total: 6_291_456,
            },
        ),
        vec![Effect::Render]
    );
    assert!(
        handle(
            &mut state,
            UpdateMsg::DownloadProgress {
                token,
                received: 65_537,
                total: 6_291_456,
            },
        )
        .is_empty()
    );
    // An older download's answer is dropped.
    assert!(
        handle(
            &mut state,
            UpdateMsg::Downloaded {
                token: token + 100,
                result: Box::new(Ok(PathBuf::from("x"))),
            },
        )
        .is_empty()
    );
    let effects = handle(
        &mut state,
        UpdateMsg::Downloaded {
            token,
            result: Box::new(Ok(PathBuf::from(r"C:\cache\MKLM-Setup-0.2.1-x64.exe"))),
        },
    );
    assert_eq!(page_state(&state), PageState::Ready);
    // The installation is in step: older downloads go (design m5b D.11).
    assert!(effects.contains(&Effect::Update(UpdateTask::Prune {
        keep: Some("MKLM-Setup-0.2.1-x64.exe".into())
    })));
    // A failed download.
    let mut failed = installed();
    checked_offer(&mut failed, offer(false, false));
    let DownloadPhase::Downloading { token, .. } = failed.update.download else {
        panic!("not downloading");
    };
    handle(
        &mut failed,
        UpdateMsg::Downloaded {
            token,
            result: Box::new(Err(DownloadError::Fetch(FetchError::HashMismatch))),
        },
    );
    assert_eq!(page_state(&failed), PageState::DownloadFailed);
}

#[test]
fn skip_and_later() {
    let mut state = ready();
    let effects = handle(&mut state, UpdateMsg::Skip);
    assert_eq!(
        state.settings.update.skipped_version.as_deref(),
        Some("0.2.1")
    );
    assert!(saves(&effects));
    assert_eq!(banner(&state), None);
    let mut later = ready();
    later.page = Page::Update;
    handle(&mut later, UpdateMsg::Later);
    assert_eq!(later.page, Page::Main);
    assert_eq!(banner(&later), None);
    // The next check shows it again.
    let (token, _) = check_task(&handle(&mut later, UpdateMsg::CheckNow)).unwrap();
    handle(
        &mut later,
        UpdateMsg::Checked {
            token,
            result: Box::new(Ok(CheckOutcome::Available(offer(true, false)))),
            client: Box::new(ClientState::default()),
        },
    );
    assert_eq!(banner(&later), Some(Banner::Available));
}

// --- "今すぐ更新" (design m5b D.2, E.4) -------------------------------------------------------

fn summary(attention: Attention, state: OpState) -> StartupSummary {
    StartupSummary {
        items: vec![AttentionItem {
            op: OpRef {
                op_id: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
                kind: OpKind::Migrate {
                    standard: Layout::Jis,
                    assignments: Vec::new(),
                },
                state,
            },
            attention,
        }],
        ..StartupSummary::default()
    }
}

#[test]
fn the_conditions_of_the_update_button() {
    assert_eq!(update_block(&ready()), None);
    let mut state = ready();
    state.update.env = Some(env(Availability::NotConfigured));
    assert_eq!(update_block(&state), Some(UpdateBlock::Unavailable));
    let mut state = installed();
    state.update.outcome = Some(CheckOutcome::Available(offer(false, false)));
    assert_eq!(update_block(&state), Some(UpdateBlock::NotReady));
    let mut state = ready();
    state.session = SessionPhase::Launching { id: 3 };
    assert_eq!(update_block(&state), Some(UpdateBlock::SessionRunning));
    for (attention, op_state, refusal) in [
        (
            Attention::AwaitingUser,
            OpState::AwaitingConfirm,
            UpdateRefusal::OperationOpen {
                waiting_for_reboot: false,
            },
        ),
        (
            Attention::WaitingForReboot,
            OpState::PendingReboot,
            UpdateRefusal::OperationOpen {
                waiting_for_reboot: true,
            },
        ),
        (
            Attention::Recover,
            OpState::Written,
            UpdateRefusal::RecoveryNeeded,
        ),
    ] {
        let mut state = ready();
        state.read.as_mut().unwrap().summary = summary(attention, op_state);
        assert_eq!(update_block(&state), Some(UpdateBlock::Journal(refusal)));
    }
    let mut state = ready();
    state.read.as_mut().unwrap().summary.unreadable = 1;
    assert_eq!(
        update_block(&state),
        Some(UpdateBlock::Journal(UpdateRefusal::JournalUnreadable))
    );
    let mut state = ready();
    state.update.machine.run = RunView::InProgress {
        phase: RunPhase::Staged,
        to_version: "0.2.1".into(),
    };
    assert_eq!(update_block(&state), Some(UpdateBlock::UpdateRunning));
    // Blocked: nothing happens.
    assert!(handle(&mut state, UpdateMsg::UpdateNow).is_empty());
    assert_eq!(state.session, SessionPhase::Idle);
}

fn undo() -> AppMsg {
    AppMsg::StartRequest {
        request: mklm_ipc::Request::Undo {
            apply: Default::default(),
        },
        apply: Default::default(),
    }
}

/// One session at a time: an update and a keyboard change exclude each other (design m3 A.4).
#[test]
fn updates_and_changes_do_not_overlap() {
    let mut state = ready();
    let effects = handle(&mut state, UpdateMsg::UpdateNow);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartUpdateSession { .. }))
    );
    // A keyboard request meanwhile does nothing.
    assert!(crate::state::update(&mut state, undo()).is_empty());
    // And an update during a change does nothing either.
    let mut changing = ready();
    crate::state::update(&mut changing, undo());
    assert_eq!(changing.session_purpose, Some(SessionPurpose::Change));
    assert!(handle(&mut changing, UpdateMsg::UpdateNow).is_empty());
}

/// `session_purpose` (FIX-VERIFICATION-11): set with `Launching`, decides where `Connected`
/// goes, and ends with the session.
#[test]
fn the_session_s_purpose() {
    let mut state = ready();
    handle(&mut state, UpdateMsg::UpdateNow);
    assert_eq!(state.session, SessionPhase::Launching { id: 0 });
    assert_eq!(state.session_purpose, Some(SessionPurpose::Update));
    crate::state::update(
        &mut state,
        AppMsg::SessionNotice {
            session: 0,
            notice: Notice::Connected(SessionKind::Request),
        },
    );
    assert_eq!(
        state.session,
        SessionPhase::Updating {
            id: 0,
            stage: UpdateStage::Sending {
                sent: 0,
                total: 6_291_456
            }
        }
    );
    handle(
        &mut state,
        UpdateMsg::SessionEnded {
            session: 0,
            end: Box::new(UpdateSessionEnd::Stage(StageEnd::Refused(
                UpdateRefusal::Busy,
            ))),
        },
    );
    assert_eq!(state.session, SessionPhase::Idle);
    assert_eq!(state.session_purpose, None);
    assert_eq!(page_state(&state), PageState::SessionFailed);
    // A change goes to `Running`.
    let mut change = installed();
    crate::state::update(&mut change, undo());
    assert_eq!(change.session_purpose, Some(SessionPurpose::Change));
    crate::state::update(
        &mut change,
        AppMsg::SessionNotice {
            session: 0,
            notice: Notice::Connected(SessionKind::Request),
        },
    );
    assert!(matches!(
        change.session,
        SessionPhase::Running { id: 0, .. }
    ));
}

/// The standalone UAC explanation before the first prompt, and where its buttons go (E.4).
#[test]
fn the_uac_explanation_of_an_update() {
    let mut state = ready();
    state.settings.change.uac_notice_seen = false;
    state.page = Page::Update;
    handle(&mut state, UpdateMsg::UpdateNow);
    assert_eq!(state.page, Page::UacNotice);
    assert_eq!(state.uac_origin, UacNoticeOrigin::Update);
    assert_eq!(state.session, SessionPhase::Idle);
    // "キャンセル": back to the update page.
    crate::state::update(&mut state, AppMsg::CancelChange);
    assert_eq!(state.page, Page::Update);
    assert_eq!(state.uac_origin, UacNoticeOrigin::Change);
    // "続ける": the session, and the explanation is seen.
    handle(&mut state, UpdateMsg::UpdateNow);
    let effects = crate::state::update(&mut state, AppMsg::UacGo);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartUpdateSession { .. }))
    );
    assert!(state.settings.change.uac_notice_seen);
    assert_eq!(state.page, Page::Update);
    // An elevated GUI explains nothing.
    let mut elevated = ready();
    elevated.settings.change.uac_notice_seen = false;
    elevated.elevated = true;
    let effects = handle(&mut elevated, UpdateMsg::UpdateNow);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::StartUpdateSession { .. }))
    );
}

// --- The hand-off (design m5b E.4; FIX-VERIFICATION-13) --------------------------------------

fn connected(state: &mut AppState) {
    handle(state, UpdateMsg::UpdateNow);
    crate::state::update(
        state,
        AppMsg::SessionNotice {
            session: 0,
            notice: Notice::Connected(SessionKind::Request),
        },
    );
}

fn handed_off(state: &mut AppState) -> Vec<Effect> {
    handle(
        state,
        UpdateMsg::SessionEnded {
            session: 0,
            end: Box::new(UpdateSessionEnd::Stage(StageEnd::HandedOff {
                run_id: RUN.into(),
                to_version: "0.2.1".into(),
            })),
        },
    )
}

#[test]
fn a_hand_off_quits_after_ok_or_15_seconds() {
    // The overlay closes before the runner stops waiting for this process.
    assert!(HANDOFF_OVERLAY_MAX + Duration::from_secs(2) < CALLER_EXIT_WAIT);
    assert_eq!(HANDOFF_OVERLAY_MAX, Duration::from_secs(15));
    let mut state = ready();
    connected(&mut state);
    let effects = handed_off(&mut state);
    assert_eq!(state.settings.update.started_run.as_deref(), Some(RUN));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::SaveSettings(s) if s.update.started_run.as_deref() == Some(RUN)
    )));
    assert!(effects.contains(&Effect::AfterUpdateRunOnce(true)));
    assert!(effects.contains(&Effect::HandOffTimer(HANDOFF_OVERLAY_MAX)));
    assert_eq!(state.overlay, OverlayKind::UpdateHandOff);
    assert!(!effects.contains(&Effect::Quit));
    // "OK" (or the timer): the normal quit.
    assert_eq!(
        handle(&mut state, UpdateMsg::HandOffClosed),
        vec![Effect::Quit]
    );
    // An elevated GUI does not register the RunOnce value (design m5b D.10).
    let mut elevated = ready();
    elevated.elevated = true;
    connected(&mut elevated);
    assert!(!handed_off(&mut elevated).contains(&Effect::AfterUpdateRunOnce(true)));
    // A late "OK" does nothing.
    let mut idle = ready();
    assert!(handle(&mut idle, UpdateMsg::HandOffClosed).is_empty());
}

#[test]
fn progress_and_cancelling_until_the_last_byte() {
    let mut state = ready();
    connected(&mut state);
    let render = handle(
        &mut state,
        UpdateMsg::SessionProgress {
            session: 0,
            progress: SessionProgress::Sent {
                bytes: 3_145_728,
                total: 6_291_456,
            },
        },
    );
    assert_eq!(render, vec![Effect::Render]);
    assert_eq!(
        handle(&mut state, UpdateMsg::Cancel),
        vec![Effect::CancelSession]
    );
    handle(
        &mut state,
        UpdateMsg::SessionProgress {
            session: 0,
            progress: SessionProgress::StartingRunner,
        },
    );
    assert_eq!(
        state.session,
        SessionPhase::Updating {
            id: 0,
            stage: UpdateStage::StartingRunner
        }
    );
    // Every byte sent: no cancelling any more.
    assert!(handle(&mut state, UpdateMsg::Cancel).is_empty());
    // A cancelled session says so and changes nothing else.
    let mut cancelled = ready();
    connected(&mut cancelled);
    handle(
        &mut cancelled,
        UpdateMsg::SessionEnded {
            session: 0,
            end: Box::new(UpdateSessionEnd::Stage(StageEnd::Cancelled)),
        },
    );
    assert_eq!(cancelled.update.note, Some(UpdateNote::Cancelled));
    assert_eq!(cancelled.update.session_end, None);
    assert_eq!(page_state(&cancelled), PageState::Ready);
    // A changed file is downloaded again.
    let mut changed = ready();
    connected(&mut changed);
    handle(
        &mut changed,
        UpdateMsg::SessionEnded {
            session: 0,
            end: Box::new(UpdateSessionEnd::Stage(StageEnd::SourceChanged)),
        },
    );
    assert!(ready_offer(&changed).is_none());
}

// --- quit-if-idle (design m5b E.4.1; RELIABILITY-1, OPS-UX-TEST-4, FIX-VERIFICATION-8) -------

#[test]
fn quit_if_idle_quits_only_an_idle_mklm() {
    let mut state = installed();
    let now = Instant::now();
    state.now = Some(now);
    let (effects, reply) = quit_if_idle(&mut state, now);
    assert_eq!(reply, InstanceReply::Ok);
    assert!(effects.contains(&Effect::AfterUpdateRunOnce(true)));
    assert!(effects.contains(&Effect::Quit));
    assert_eq!(state.settings.update.closed_by_update, Some(NOW));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::SaveSettings(s) if s.update.closed_by_update == Some(NOW)
    )));
    // Elevated: no RunOnce value (it may be another administrator's HKCU).
    let mut elevated = installed();
    elevated.elevated = true;
    elevated.now = Some(now);
    let (effects, reply) = quit_if_idle(&mut elevated, now);
    assert_eq!(reply, InstanceReply::Ok);
    assert!(!effects.contains(&Effect::AfterUpdateRunOnce(true)));
}

/// Busy never changes anything: no cancel, no pending quit, no question (FIX-VERIFICATION-8).
#[test]
fn busy_changes_nothing() {
    let op = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
    let countdown = SessionView {
        prompt: Prompt::Countdown {
            op_id: op.clone(),
            seconds: 20,
            remaining: 12,
            verified: true,
        },
        ..SessionView::default()
    };
    let reconnect = SessionView {
        prompt: Prompt::Reconnect {
            op_id: op,
            instance_ids: vec!["X".into()],
            reminders: 1,
        },
        ..SessionView::default()
    };
    let mut busy: Vec<AppState> = Vec::new();
    let mut with = |change: &dyn Fn(&mut AppState)| {
        let mut state = ready();
        change(&mut state);
        busy.push(state);
    };
    with(&|s| s.session = SessionPhase::Launching { id: 1 });
    with(&|s| {
        s.session = SessionPhase::Launching { id: 1 };
        s.session_purpose = Some(SessionPurpose::Update);
    });
    with(&|s| {
        s.session = SessionPhase::Running {
            id: 1,
            view: Box::new(countdown.clone()),
        };
        s.overlay = OverlayKind::Countdown;
    });
    with(&|s| {
        s.session = SessionPhase::Running {
            id: 1,
            view: Box::new(reconnect.clone()),
        };
        s.overlay = OverlayKind::Reconnect;
    });
    with(&|s| {
        s.session = SessionPhase::Running {
            id: 1,
            view: Box::default(),
        };
        s.overlay = OverlayKind::Progress;
    });
    for stage in [
        UpdateStage::Sending { sent: 1, total: 2 },
        UpdateStage::StartingRunner,
        UpdateStage::HandedOff,
    ] {
        with(&move |s| {
            s.session = SessionPhase::Updating { id: 1, stage };
            s.session_purpose = Some(SessionPurpose::Update);
        });
    }
    with(&|s| s.quit_pending = true);
    with(&|s| s.overlay = OverlayKind::QuitConfirm);
    with(&|s| s.overlay = OverlayKind::RecoveryConfirm);
    for mut state in busy {
        let now = Instant::now();
        state.now = Some(now);
        let before = state.clone();
        let (effects, reply) = quit_if_idle(&mut state, now);
        assert_eq!(reply, InstanceReply::Busy, "{:?}", before.session);
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(state, before);
    }
}

#[test]
fn a_late_quit_if_idle_does_nothing() {
    let mut state = installed();
    let received = Instant::now();
    state.now = Some(received + Duration::from_secs(5));
    let before = state.clone();
    let (effects, reply) = quit_if_idle(&mut state, received);
    assert!(effects.is_empty());
    assert_eq!(reply, InstanceReply::Busy);
    assert_eq!(state, before);
}

// --- The table of E.4.1 ----------------------------------------------------------------------

/// The four stages of an update session.
fn stages() -> Vec<(&'static str, AppState)> {
    let mut launching = ready();
    handle(&mut launching, UpdateMsg::UpdateNow);
    let mut sending = ready();
    connected(&mut sending);
    let mut starting = ready();
    connected(&mut starting);
    handle(
        &mut starting,
        UpdateMsg::SessionProgress {
            session: 0,
            progress: SessionProgress::StartingRunner,
        },
    );
    let mut handed = ready();
    connected(&mut handed);
    handed_off(&mut handed);
    vec![
        ("launching", launching),
        ("sending", sending),
        ("starting", starting),
        ("handed off", handed),
    ]
}

#[test]
fn the_window_s_close_button() {
    for (name, mut state) in stages() {
        let effects = crate::state::update(&mut state, AppMsg::WindowCloseRequested);
        if name == "handed off" {
            assert_eq!(effects, vec![Effect::Quit], "{name}");
        } else {
            assert!(effects.is_empty(), "{name}: {effects:?}");
        }
    }
}

#[test]
fn quitting_during_an_update() {
    let mut stages = stages().into_iter();
    // UAC: at once (the cancel flag too).
    let (_, mut launching) = stages.next().unwrap();
    assert!(!crate::state::quit_reply_waits(&launching));
    assert_eq!(
        crate::state::update(&mut launching, AppMsg::QuitRequested),
        vec![Effect::CancelSession, Effect::Quit]
    );
    // Sending: stop and quit once the session has ended; `ok`.
    let (_, mut sending) = stages.next().unwrap();
    assert!(!crate::state::quit_reply_waits(&sending));
    assert_eq!(
        crate::state::update(&mut sending, AppMsg::QuitRequested),
        vec![Effect::CancelSession]
    );
    assert!(sending.quit_pending);
    let effects = handle(
        &mut sending,
        UpdateMsg::SessionEnded {
            session: 0,
            end: Box::new(UpdateSessionEnd::Stage(StageEnd::Cancelled)),
        },
    );
    assert!(effects.contains(&Effect::Quit));
    // The runner starting: later (`busy`); after the hand-off, or a refusal.
    let (_, mut starting) = stages.next().unwrap();
    assert!(crate::state::quit_reply_waits(&starting));
    assert!(crate::state::update(&mut starting, AppMsg::QuitRequested).is_empty());
    assert!(starting.quit_pending);
    let mut refused = starting.clone();
    assert!(handed_off(&mut starting).contains(&Effect::Quit));
    let effects = handle(
        &mut refused,
        UpdateMsg::SessionEnded {
            session: 0,
            end: Box::new(UpdateSessionEnd::Stage(StageEnd::Refused(
                UpdateRefusal::HandOffFailed {
                    detail: "exit code 7".into(),
                },
            ))),
        },
    );
    assert!(effects.contains(&Effect::Quit));
    // Handed off: at once.
    let (_, mut handed) = stages.next().unwrap();
    assert!(!crate::state::quit_reply_waits(&handed));
    assert_eq!(
        crate::state::update(&mut handed, AppMsg::QuitRequested),
        vec![Effect::Quit]
    );
}

#[test]
fn activate_always_shows_the_window() {
    for (name, mut state) in stages() {
        state.visible = false;
        let effects = crate::state::update(&mut state, AppMsg::Activate);
        assert!(effects.contains(&Effect::ShowWindow), "{name}");
        assert!(state.visible);
    }
}

// --- The start (design m5b D.13 step 1; FIX-VERIFICATION-9) ----------------------------------

#[test]
fn a_running_update_keeps_mklm_from_starting() {
    let running = |phase| RunView::InProgress {
        phase,
        to_version: "0.2.1".into(),
    };
    for phase in [
        RunPhase::Ready,
        RunPhase::Waiting,
        RunPhase::Installing,
        RunPhase::Finishing,
    ] {
        assert_eq!(
            start_gate(&running(phase), Some(1), Some(1)),
            Some(StartGate::QuitQuietly)
        );
        assert_eq!(
            start_gate(&running(phase), Some(1), Some(2)),
            Some(StartGate::QuitAndRemember)
        );
        assert_eq!(
            start_gate(&running(phase), None, Some(2)),
            Some(StartGate::QuitAndRemember)
        );
    }
    // While the GUI that asked still runs (receiving), or nothing runs: a normal start.
    for phase in [RunPhase::Staging, RunPhase::Staged] {
        assert_eq!(start_gate(&running(phase), Some(1), Some(2)), None);
    }
    assert_eq!(start_gate(&RunView::Idle, Some(1), Some(1)), None);
}

// --- What a start shows (design m5b D.13, E.5; OPS-UX-TEST-6, RELIABILITY-7) -----------------

fn result(outcome: UpdateOutcome, finished: u64) -> UpdateResult {
    UpdateResult {
        schema: 1,
        run_id: RunId::parse(RUN).unwrap(),
        from_version: "0.2.0".into(),
        to_version: "0.2.1".into(),
        arch: Arch::X64,
        finished_at: Timestamp(finished * 1000),
        outcome,
        installer_exit: Some(0),
        installed_version: Some("0.2.1".into()),
        gui_relaunch_attempted: true,
    }
}

/// MKLM 0.2.1 running after the update.
fn updated() -> AppState {
    let mut state = installed();
    let mut env = env(Availability::Available);
    env.installed = Version::new(0, 2, 1);
    state.update.env = Some(env);
    state.update.machine.consistent = Some(Version::new(0, 2, 1));
    state
}

fn machine_with(last: UpdateResult, consistent: Option<Version>) -> MachineView {
    MachineView {
        last_result: Some(last),
        consistent,
        install_known: true,
        ..MachineView::default()
    }
}

#[test]
fn this_user_s_update_is_shown_once() {
    let mut state = updated();
    state.settings.update.started_run = Some(RUN.into());
    let machine = machine_with(
        result(UpdateOutcome::Installed, NOW - 60),
        Some(Version::new(0, 2, 1)),
    );
    let effects = start(&mut state, machine.clone(), ClientState::default());
    assert_eq!(state.overlay, OverlayKind::UpdateResult);
    assert!(matches!(
        &state.update.shown,
        Some(ShownResult::Result {
            own: true,
            closed_for_update: false,
            ..
        })
    ));
    assert_eq!(state.settings.update.result_seen.as_deref(), Some(RUN));
    assert!(effects.contains(&Effect::ShowWindow));
    // The installer in the cache goes: the update to the running version is complete (D.11).
    assert!(effects.contains(&Effect::Update(UpdateTask::Prune { keep: None })));
    assert!(!effects.contains(&Effect::AfterUpdateRunOnce(false)));
    // Seen: the next start shows nothing and removes the RunOnce value (D.13 step 5).
    let mut again = updated();
    again.settings = state.settings.clone();
    let effects = start(&mut again, machine, ClientState::default());
    assert_eq!(again.update.shown, None);
    assert!(effects.contains(&Effect::AfterUpdateRunOnce(false)));
    // An elevated GUI leaves the value alone (it may be another administrator's HKCU).
    let mut elevated = updated();
    elevated.elevated = true;
    let effects = start(
        &mut elevated,
        MachineView::default(),
        ClientState::default(),
    );
    assert!(!effects.contains(&Effect::AfterUpdateRunOnce(false)));
}

#[test]
fn what_is_not_shown() {
    let installed_result = result(UpdateOutcome::Installed, NOW - 60);
    let settings = crate::settings::UpdateSettings {
        started_run: Some(RUN.into()),
        ..crate::settings::UpdateSettings::default()
    };
    let running_021 = updated().update.env.unwrap();
    let running_020 = env(Availability::Available);
    // Older than 14 days: marked seen only.
    let old = result(UpdateOutcome::Installed, NOW - (RESULT_SHOW_DAYS + 1) * DAY);
    let (shown, seen) = result_to_show(
        &settings,
        &machine_with(old, Some(Version::new(0, 2, 1))),
        &running_021,
        None,
        false,
        NOW,
    );
    assert_eq!((shown, seen.as_deref()), (None, Some(RUN)));
    // Installed, but another version runs now (reinstalled since): not shown.
    let (shown, _) = result_to_show(
        &settings,
        &machine_with(installed_result.clone(), Some(Version::new(0, 2, 0))),
        &running_020,
        None,
        false,
        NOW,
    );
    assert_eq!(shown, None);
    // Another user's update: the neutral text; its failures are not shown.
    let other = crate::settings::UpdateSettings::default();
    let (shown, _) = result_to_show(
        &other,
        &machine_with(installed_result, Some(Version::new(0, 2, 1))),
        &running_021,
        None,
        false,
        NOW,
    );
    assert!(matches!(
        shown,
        Some(ShownResult::Result { own: false, .. })
    ));
    let busy = result(
        UpdateOutcome::NotInstalled(NotInstalledReason::InstanceBusy { sessions: vec![2] }),
        NOW - 60,
    );
    let (shown, seen) = result_to_show(
        &other,
        &machine_with(busy.clone(), Some(Version::new(0, 2, 0))),
        &running_020,
        None,
        false,
        NOW,
    );
    assert_eq!((shown, seen.as_deref()), (None, Some(RUN)));
    // … but to this user they are.
    let (shown, _) = result_to_show(
        &settings,
        &machine_with(busy, Some(Version::new(0, 2, 0))),
        &running_020,
        None,
        false,
        NOW,
    );
    assert!(matches!(shown, Some(ShownResult::Result { own: true, .. })));
}

fn dummy_record() -> mklm_update::run::RunRecord {
    mklm_update::run::RunRecord {
        schema: 1,
        run_id: RunId::parse(RUN).unwrap(),
        from_version: "0.2.0".into(),
        to_version: "0.2.1".into(),
        arch: Arch::X64,
        phase: RunPhase::Waiting,
        boot_id: mklm_core::BootId(1),
        started_at: Timestamp(1),
        phase_at: Timestamp(2),
        caller: None,
        caller_session: Some(1),
        stager: mklm_core::ProcessIdentity {
            pid: 1,
            creation_time: 2,
        },
        runner: None,
        installer: None,
    }
}

#[test]
fn an_interruption_is_shown_once_per_user() {
    let interrupted = UpdateResult {
        outcome: UpdateOutcome::Interrupted {
            phase: RunPhase::Waiting,
        },
        installer_exit: None,
        installed_version: Some("0.2.0".into()),
        ..result(UpdateOutcome::Installed, NOW - 60)
    };
    let machine = MachineView {
        run: RunView::Interrupted(dummy_record()),
        interrupted: Some(interrupted),
        consistent: Some(Version::new(0, 2, 0)),
        install_known: true,
        ..MachineView::default()
    };
    let mut settings = crate::settings::UpdateSettings {
        started_run: Some(RUN.into()),
        ..crate::settings::UpdateSettings::default()
    };
    let running = env(Availability::Available);
    let (shown, seen) = result_to_show(&settings, &machine, &running, None, false, NOW);
    assert!(shown.is_some());
    settings.result_seen = seen;
    let (shown, seen) = result_to_show(&settings, &machine, &running, None, false, NOW);
    assert_eq!((shown, seen), (None, None));
}

/// The installed programs out of step are shown to everyone (D.13 step 4).
#[test]
fn programs_out_of_step_are_shown_to_everyone() {
    let failed = result(UpdateOutcome::Failed(FailedReason::Inconsistent), NOW - 60);
    let machine = machine_with(failed, None);
    let other = crate::settings::UpdateSettings::default();
    let (shown, _) = result_to_show(
        &other,
        &machine,
        &env(Availability::Available),
        Some(&Version::new(0, 2, 1)),
        true,
        NOW,
    );
    assert_eq!(
        shown,
        Some(ShownResult::Inconsistent {
            version: Version::new(0, 2, 1),
            cached: true
        })
    );
    let mut state = installed();
    state.update.machine.consistent = None;
    assert!(inconsistent(&state));
    assert_eq!(page_state(&state), PageState::Inconsistent);
    // A development copy reads no build IDs: never "out of step".
    state.update.env = Some(env(Availability::NotInstalledCopy {
        exe_dir: PathBuf::from(r"D:\dev"),
        install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
    }));
    assert!(!inconsistent(&state));
}

/// "別のユーザーの更新のために…" after `quit-if-idle` (E.5; RELIABILITY-7).
#[test]
fn closed_for_another_user_s_update() {
    let mut state = updated();
    state.settings.update.closed_by_update = Some(NOW - 600);
    let machine = machine_with(
        result(UpdateOutcome::Installed, NOW - 60),
        Some(Version::new(0, 2, 1)),
    );
    start(&mut state, machine, ClientState::default());
    assert!(matches!(
        &state.update.shown,
        Some(ShownResult::Result {
            own: false,
            closed_for_update: true,
            ..
        })
    ));
    assert_eq!(state.settings.update.closed_by_update, None);
}

// --- Banners (design m5b E.3; SECURITY-11, OPS-UX-TEST-5) ------------------------------------

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

#[test]
fn thirty_days_without_a_check() {
    let mut state = installed();
    let effects = start(
        &mut state,
        MachineView::default(),
        failing(ErrorClass::Transient, "upd-net", NOW - 31 * DAY),
    );
    assert_eq!(
        state.update.notice,
        Some(UpdateNotice::Stale { structural: false })
    );
    assert_eq!(state.settings.update.stale_notice_at, Some(NOW));
    assert!(saves(&effects));
    // Once every 30 days.
    let mut again = installed();
    again.settings.update.stale_notice_at = Some(NOW - 10 * DAY);
    start(
        &mut again,
        MachineView::default(),
        failing(ErrorClass::Transient, "upd-net", NOW - 31 * DAY),
    );
    assert_eq!(again.update.notice, None);
    // Structural.
    let mut structural = installed();
    start(
        &mut structural,
        MachineView::default(),
        failing(ErrorClass::Structural, "upd-gh-changed", NOW - 40 * DAY),
    );
    assert_eq!(
        structural.update.notice,
        Some(UpdateNotice::Stale { structural: true })
    );
    // 29 days: nothing yet; automatic checks off: nothing.
    let mut recent = installed();
    start(
        &mut recent,
        MachineView::default(),
        failing(ErrorClass::Transient, "upd-net", NOW - 29 * DAY),
    );
    assert_eq!(recent.update.notice, None);
    let mut off = installed();
    off.settings.update.auto_check = false;
    start(
        &mut off,
        MachineView::default(),
        failing(ErrorClass::Transient, "upd-net", NOW - 31 * DAY),
    );
    assert_eq!(off.update.notice, None);
    // Opening the page takes the banner away.
    crate::state::update(&mut state, AppMsg::Navigate(Page::Update));
    assert_eq!(state.update.notice, None);
}

#[test]
fn an_expired_manifest() {
    let mut state = installed();
    state.now_unix = Some(ISSUED + 181 * DAY);
    let mut old = verified("0.2.0", OfferKind::UpToDate);
    old.freshness = Freshness::Expired;
    let env = state.update.env.clone().unwrap();
    handle(
        &mut state,
        UpdateMsg::Started(Box::new(UpdateStart {
            env,
            machine: MachineView::default(),
            client: ClientState {
                last_check: Some(ISSUED + 181 * DAY),
                last_success: Some(ISSUED + 181 * DAY),
                ..ClientState::default()
            },
            cached: Some(Ok(CheckOutcome::UpToDate(old))),
        })),
    );
    assert_eq!(
        state.update.notice,
        Some(UpdateNotice::Expired {
            expires: ISSUED + 180 * DAY
        })
    );
}

/// The rollback warning once per `issued_at`; after 30 days of rollbacks only, the structural
/// notice (FIX-VERIFICATION-12).
#[test]
fn rollback_warnings() {
    let rolled_back = |at: u64| ClientState {
        last_rollback: Some(RollbackNote {
            issued_at: ISSUED - DAY,
            seen: ISSUED,
            at,
        }),
        ..failing(ErrorClass::Structural, "upd-rollback", NOW - 31 * DAY)
    };
    let rollback = || {
        Box::new(Err(CheckError::Refused(UpdateRefusal::Rollback {
            issued_at: ISSUED - DAY,
            seen: ISSUED,
        })))
    };
    let mut state = installed();
    let (token, _) = check_task(&handle(&mut state, UpdateMsg::CheckDue)).unwrap();
    handle(
        &mut state,
        UpdateMsg::Checked {
            token,
            result: rollback(),
            client: Box::new(rolled_back(NOW)),
        },
    );
    assert_eq!(
        state.update.notice,
        Some(UpdateNotice::Rollback {
            issued_at: ISSUED - DAY,
            seen: ISSUED
        })
    );
    assert_eq!(
        state.settings.update.rollback_notice_for,
        Some(ISSUED - DAY)
    );
    // The page seen, the same rollback again: the structural notice now, not the warning.
    crate::state::update(&mut state, AppMsg::Navigate(Page::Update));
    let (token, _) = check_task(&handle(&mut state, UpdateMsg::CheckNow)).unwrap();
    handle(
        &mut state,
        UpdateMsg::Checked {
            token,
            result: rollback(),
            client: Box::new(rolled_back(NOW + 60)),
        },
    );
    assert_eq!(
        state.update.notice,
        Some(UpdateNotice::Stale { structural: true })
    );
}

#[test]
fn a_cancelled_check_changes_nothing() {
    let mut state = installed();
    state.update.outcome = Some(CheckOutcome::UpToDate(verified(
        "0.2.0",
        OfferKind::UpToDate,
    )));
    let (token, _) = check_task(&handle(&mut state, UpdateMsg::CheckNow)).unwrap();
    handle(
        &mut state,
        UpdateMsg::Checked {
            token,
            result: Box::new(Err(CheckError::Fetch(FetchError::Cancelled))),
            client: Box::new(ClientState::default()),
        },
    );
    assert_eq!(state.update.check_error, None);
    assert_eq!(state.update.note, Some(UpdateNote::Cancelled));
    assert_eq!(page_state(&state), PageState::UpToDate);
    // An older check's answer is dropped.
    let before = state.clone();
    assert!(
        handle(
            &mut state,
            UpdateMsg::Checked {
                token: 99,
                result: Box::new(Err(CheckError::Fetch(FetchError::NotFound))),
                client: Box::new(ClientState::default()),
            },
        )
        .is_empty()
    );
    assert_eq!(state, before);
}

#[test]
fn the_tray_item() {
    assert!(tray_can_update(&ready()));
    let mut busy = ready();
    busy.session = SessionPhase::Launching { id: 1 };
    assert!(!tray_can_update(&busy));
    assert!(!tray_can_update(&installed()));
}
