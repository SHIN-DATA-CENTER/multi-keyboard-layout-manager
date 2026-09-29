//! Updates in the GUI (design m5b E; H.4 "GUI の中の名前"): the check schedule, the automatic
//! download, the update page's state, the update session (the UAC prompt, the installer handed
//! to the helper, the hand-off), `quit-if-idle`, and what the next start shows (D.13, E.5).
//!
//! Pure, like the rest of `state`: the update worker (`update_worker.rs`) checks, downloads and
//! reads the records; a session worker runs the update session (`worker.rs`); `app.rs` runs the
//! timers. Every answer comes back as an [`UpdateMsg`]. The schedule's random part is drawn by
//! `app.rs` ([`Effect::ScheduleUpdateCheck`]'s `jitter`), so the rules here are deterministic.
//!
//! The update page resolves to one [`PageState`] (design m5b E.2), in this order: updates not
//! available; the installed programs out of step (D.13); an update session; a check; a download;
//! the last session's or download's failure; what the last verified manifest offers; the last
//! check's failure; nothing known yet.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mklm_client::orchestrator::LaunchError;
use mklm_client::startup::StartupSummary;
use mklm_client::update::cache::{ClientState, ErrorClass};
use mklm_client::update::check::{CheckError, CheckOutcome, Offer};
use mklm_client::update::download::DownloadError;
use mklm_client::update::env::Availability;
use mklm_client::update::stage::StageEnd;
use mklm_core::Attention;
use mklm_update::run::timing::{HANDOFF_OVERLAY_MAX, RESULT_SHOW_DAYS, STALE_NOTICE_DAYS};
use mklm_update::run::{RunPhase, RunView, UpdateOutcome, UpdateResult};
use mklm_update::{Freshness, UpdateRefusal, Version};

use super::{
    AppState, Effect, OverlayKind, Page, SessionId, SessionPhase, SessionPurpose, UacNoticeOrigin,
    UpdateStage,
};

/// A day, in seconds.
const DAY: u64 = 86_400;
/// Checks are 24 hours apart (design m5b E.1).
pub const CHECK_INTERVAL: Duration = Duration::from_secs(DAY);
/// The random part added to the interval: up to 60 minutes.
pub const CHECK_JITTER: Duration = Duration::from_secs(60 * 60);
/// The first check after a start when the last one is 24 hours old: 60 s plus up to 120 s.
pub const START_DELAY: Duration = Duration::from_secs(60);
pub const START_JITTER: Duration = Duration::from_secs(120);
/// After a resume past the schedule.
pub const RESUME_DELAY: Duration = Duration::from_secs(30);
/// A time this far in the future is a clock that ran ahead (RELIABILITY-8).
pub const FUTURE_TOLERANCE: u64 = 60 * 60;
/// While an update session runs, an automatic check waits this long.
pub const BUSY_RETRY: Duration = Duration::from_secs(60 * 60);
/// A `quit-if-idle` that reaches the UI thread later than this after the pipe thread received it
/// is ignored: the runner no longer waits for the answer (`single_instance::UI_REPLY_WAIT`).
pub const QUIT_IF_IDLE_WAIT: Duration = Duration::from_secs(4);

/// The single-instance pipe's answer to `quit-if-idle` (mapped to `mklm_win::instance`'s by
/// `app.rs`, so that this module stays free of Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceReply {
    Ok,
    Busy,
}

/// This installation as the update worker found it at start (`mklm_client::update::env`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvView {
    pub installed: Version,
    pub availability: Availability,
    /// `%ProgramFiles%\SHIN DATA CENTER\MKLM` (the UAC explanation names the helper in it).
    pub install_dir: PathBuf,
    /// An ARM64 PC running this x64 build (shown only, design m5b C.7).
    pub arm64_pc: bool,
}

/// The machine's update records as last read (`mklm_client::update::status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineView {
    pub run: RunView,
    pub last_result: Option<UpdateResult>,
    /// The result an interrupted `Run` stands for (`run::interrupted_result`), when `run` is
    /// `Interrupted`: the GUI cannot write HKLM, so it shows it from `Run` (design m5b D.13).
    pub interrupted: Option<UpdateResult>,
    /// The version all three installed programs have (`InstallState::consistent_version`).
    pub consistent: Option<Version>,
    /// Some build ID could be read at all (a development copy reads none).
    pub install_known: bool,
}

impl Default for MachineView {
    fn default() -> Self {
        Self {
            run: RunView::Idle,
            last_result: None,
            interrupted: None,
            consistent: None,
            install_known: false,
        }
    }
}

/// What the update worker read at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStart {
    pub env: EnvView,
    pub machine: MachineView,
    pub client: ClientState,
    /// The cached manifest verified again (`reverify_cached`); `None` when none is cached.
    pub cached: Option<Result<CheckOutcome, CheckError>>,
}

/// A job for the update worker (design m5b E.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateTask {
    /// The environment, the machine's and the user's records, the cached manifest; removes the
    /// downloads left under way. Answered by [`UpdateMsg::Started`].
    Start,
    Check {
        token: u64,
        manual: bool,
        skipped: Option<Version>,
    },
    Download {
        token: u64,
        offer: Box<Offer>,
    },
    /// The machine's records again ([`UpdateMsg::StatusRead`]).
    ReadStatus,
    /// `UpdateCache::prune` (design m5b D.11).
    Prune {
        keep: Option<String>,
    },
    /// `mklm_win::ui::open_release_page`.
    OpenReleasePage(String),
    /// The cached manifest verified again, the installer's size and SHA-256 compared, then the
    /// interactive installer with UAC (design m5b D.13 step 4; RED-TEAM-2: not a verification
    /// that protects anything). Answered by [`UpdateMsg::InstallerRan`].
    RunInstaller,
}

/// The progress of an update session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProgress {
    Sent { bytes: u64, total: u64 },
    StartingRunner,
}

/// How an update session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateSessionEnd {
    /// No helper came up (the UAC prompt declined: `LaunchError::Declined`).
    NotLaunched(LaunchError),
    /// The cached manifest or installer did not pass again before the prompt (design m5b D.2).
    NotReady(CheckError),
    Stage(StageEnd),
}

/// Something the update worker or a session worker reports, or a button.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateMsg {
    Started(Box<UpdateStart>),
    StatusRead(Box<MachineView>),
    /// The timer of the next automatic check.
    CheckDue,
    /// "今すぐ確認" / "もう一度確認".
    CheckNow,
    /// "キャンセル" of a check, a download or an update session.
    Cancel,
    Checked {
        token: u64,
        result: Box<Result<CheckOutcome, CheckError>>,
        client: Box<ClientState>,
    },
    /// "ダウンロード" / "もう一度ダウンロード".
    Download,
    DownloadProgress {
        token: u64,
        received: u64,
        total: u64,
    },
    Downloaded {
        token: u64,
        result: Box<Result<PathBuf, DownloadError>>,
    },
    Skip,
    Later,
    UpdateNow,
    /// The update page (banner "詳細…", the tray, the settings page).
    OpenPage,
    OpenReleaseNotes,
    OpenReleasePage,
    CopyDetails,
    RunInstaller,
    InstallerRan(Result<(), InstallerRunError>),
    /// "更新を自動で確認する".
    AutoCheck(bool),
    /// The PC woke up (`ShellEvent::Resumed`).
    Resumed,
    SessionProgress {
        session: SessionId,
        progress: SessionProgress,
    },
    SessionEnded {
        session: SessionId,
        end: Box<UpdateSessionEnd>,
    },
    /// The hand-off overlay's "OK", or its time is up.
    HandOffClosed,
    ResultClosed,
    /// "確認…" of a refusal for an open change: the journal's banner page.
    Review,
    /// "再起動…" of a refusal for a pending restart.
    RestartPage,
}

/// The check under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckPhase {
    #[default]
    Idle,
    Checking {
        token: u64,
        manual: bool,
    },
}

/// The download under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadPhase {
    #[default]
    Idle,
    Downloading {
        token: u64,
        received: u64,
        total: u64,
    },
}

/// A banner that is shown until the page is opened (design m5b E.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateNotice {
    /// No successful check for 30 days: the last failure's class.
    Stale { structural: bool },
    /// The cached manifest expired (B.4 5).
    Expired { expires: u64 },
    /// An older manifest was ignored (SECURITY-11).
    Rollback { issued_at: u64, seen: u64 },
}

/// What the result overlay shows (design m5b D.13, E.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShownResult {
    /// A finished (or interrupted) update: this user's (`own`) or another user's.
    Result {
        result: Box<UpdateResult>,
        own: bool,
        /// This MKLM had quit (or not started) for another user's update (E.5).
        closed_for_update: bool,
    },
    /// The three installed programs are of different versions now (D.13 step 4): the version
    /// whose installer fixes it, and whether the cache holds that installer.
    Inconsistent { version: Version, cached: bool },
}

/// Why "インストーラーを実行" did not start the installer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallerRunError {
    /// The UAC prompt was declined.
    Cancelled,
    /// The cached files did not pass again, or Windows refused (English detail).
    Failed(String),
}

/// A short message on the page (not a failure of the offer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateNote {
    Cancelled,
    /// The interactive installer could not be started (English detail).
    InstallerFailed(String),
}

/// Everything the GUI keeps about updates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateState {
    pub env: Option<EnvView>,
    pub machine: MachineView,
    /// The user's record (`state.json`) as the worker last read it.
    pub client: ClientState,
    /// What the last verified manifest offers (a check, or the cache verified again at start).
    pub outcome: Option<CheckOutcome>,
    /// The last check's failure; `None` after a success.
    pub check_error: Option<CheckError>,
    pub check: CheckPhase,
    pub download: DownloadPhase,
    pub download_error: Option<DownloadError>,
    /// The last update session, when it did not hand off.
    pub session_end: Option<UpdateSessionEnd>,
    pub note: Option<UpdateNote>,
    pub next_token: u64,
    /// The earliest the next automatic check is due (Unix seconds).
    pub next_check_at: Option<u64>,
    /// "後で": the "ready" banner is hidden until the next check or start.
    pub banner_later: bool,
    pub notice: Option<UpdateNotice>,
    pub shown: Option<ShownResult>,
    /// Started with `--after-update`.
    pub after_update: bool,
}

/// What the update page shows (design m5b E.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// `NotConfigured` / `Unknown` (a development copy checks: see [`Availability`]).
    Unavailable,
    /// The installed programs are of different versions (D.13).
    Inconsistent,
    /// The update session (the UAC prompt, the hand-over, the runner).
    Session,
    Checking,
    Downloading,
    /// The last update session did not hand off.
    SessionFailed,
    DownloadFailed,
    /// Downloaded and verified: "今すぐ更新".
    Ready,
    /// Available, skipped, not downloaded.
    Skipped,
    /// Available, not downloaded (the automatic check is off, or the download did not start).
    NotDownloaded,
    Manual,
    UpToDate,
    CheckFailed,
    NotChecked,
}

impl AppState {
    fn now_unix(&self) -> u64 {
        self.now_unix.unwrap_or(0)
    }
}

/// The installed version (`CARGO_PKG_VERSION` until the worker says).
pub fn installed(state: &AppState) -> Version {
    state.update.env.as_ref().map_or_else(
        || Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0)),
        |env| env.installed.clone(),
    )
}

/// The availability of updates (`Unknown` until the worker has started).
pub fn availability(state: &AppState) -> Availability {
    state.update.env.as_ref().map_or(
        Availability::Unknown {
            detail: "not read yet".into(),
        },
        |env| env.availability.clone(),
    )
}

/// A check is possible (design m5b E.2, E.8): a development copy checks too.
pub fn can_check(state: &AppState) -> bool {
    matches!(
        availability(state),
        Availability::Available | Availability::NotInstalledCopy { .. }
    )
}

/// The last verified manifest, whatever it offers.
pub fn cached_verified(state: &AppState) -> Option<&mklm_update::VerifiedManifest> {
    state.update.outcome.as_ref().map(CheckOutcome::verified)
}

/// The offer on the table, if any.
pub fn offer(state: &AppState) -> Option<&Offer> {
    match &state.update.outcome {
        Some(CheckOutcome::Available(offer)) => Some(offer),
        _ => None,
    }
}

/// The offer when it is downloaded and verified.
pub fn ready_offer(state: &AppState) -> Option<&Offer> {
    offer(state).filter(|offer| offer.downloaded.is_some())
}

/// The installed programs are of different versions (design m5b D.13 step 4): only for an
/// installed MKLM whose build IDs could be read.
pub fn inconsistent(state: &AppState) -> bool {
    matches!(availability(state), Availability::Available)
        && state.update.machine.install_known
        && state.update.machine.consistent.is_none()
}

/// An update session: the UAC prompt of an update, or later.
pub fn in_update_session(state: &AppState) -> bool {
    match state.session {
        SessionPhase::Updating { .. } => true,
        SessionPhase::Launching { .. } => state.session_purpose == Some(SessionPurpose::Update),
        SessionPhase::Idle | SessionPhase::Running { .. } => false,
    }
}

/// What stops "今すぐ更新" before the UAC prompt (design m5b D.2; the helper decides again).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateBlock {
    /// Condition 1: updates are not available here.
    Unavailable,
    /// Condition 2: nothing downloaded to install.
    NotReady,
    /// Condition 3: a helper session runs.
    SessionRunning,
    /// Condition 4: the journal has an open operation.
    Journal(UpdateRefusal),
    /// Condition 5: another update runs.
    UpdateRunning,
}

/// The journal's condition from the last read (design m5b D.2 condition 4: a look ahead, so
/// that no UAC prompt ends in a refusal; `gate::check_journal` is the helper's rule).
pub fn journal_block(summary: &StartupSummary) -> Option<UpdateRefusal> {
    if summary.unreadable > 0 {
        return Some(UpdateRefusal::JournalUnreadable);
    }
    let mut open = None;
    for item in &summary.items {
        match item.attention {
            Attention::None | Attention::NeedsApply => {}
            Attention::Busy => return Some(UpdateRefusal::Busy),
            Attention::Recover => return Some(UpdateRefusal::RecoveryNeeded),
            Attention::WaitingForReboot => {
                open = Some(UpdateRefusal::OperationOpen {
                    waiting_for_reboot: true,
                });
            }
            Attention::AwaitingUser | Attention::Conflict => {
                open.get_or_insert(UpdateRefusal::OperationOpen {
                    waiting_for_reboot: false,
                });
            }
        }
    }
    open
}

/// Why "今すぐ更新" cannot be pressed now (design m5b D.2); `None` when it can.
pub fn update_block(state: &AppState) -> Option<UpdateBlock> {
    if availability(state) != Availability::Available {
        return Some(UpdateBlock::Unavailable);
    }
    if ready_offer(state).is_none() {
        return Some(UpdateBlock::NotReady);
    }
    if state.session != SessionPhase::Idle {
        return Some(UpdateBlock::SessionRunning);
    }
    if let Some(refusal) = state
        .read
        .as_ref()
        .and_then(|read| journal_block(&read.summary))
    {
        return Some(UpdateBlock::Journal(refusal));
    }
    if matches!(state.update.machine.run, RunView::InProgress { .. }) {
        return Some(UpdateBlock::UpdateRunning);
    }
    None
}

/// The page's state (design m5b E.2; the order of the module's documentation).
pub fn page_state(state: &AppState) -> PageState {
    let update = &state.update;
    if matches!(
        availability(state),
        Availability::NotConfigured | Availability::Unknown { .. }
    ) {
        return PageState::Unavailable;
    }
    if inconsistent(state) {
        return PageState::Inconsistent;
    }
    if in_update_session(state) {
        return PageState::Session;
    }
    if matches!(update.check, CheckPhase::Checking { .. }) {
        return PageState::Checking;
    }
    if matches!(update.download, DownloadPhase::Downloading { .. }) {
        return PageState::Downloading;
    }
    if update.session_end.is_some() {
        return PageState::SessionFailed;
    }
    if update.download_error.is_some() {
        return PageState::DownloadFailed;
    }
    match &update.outcome {
        Some(CheckOutcome::Available(offer)) => {
            return if offer.downloaded.is_some() {
                PageState::Ready
            } else if offer.skipped {
                PageState::Skipped
            } else {
                PageState::NotDownloaded
            };
        }
        Some(CheckOutcome::ManualRequired(_)) => return PageState::Manual,
        Some(CheckOutcome::UpToDate(_)) => return PageState::UpToDate,
        None => {}
    }
    if update.check_error.is_some() {
        PageState::CheckFailed
    } else {
        PageState::NotChecked
    }
}

/// The banner of the main screen (design m5b E.3): a notice first, then a ready update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    Notice(UpdateNotice),
    Available,
}

pub fn banner(state: &AppState) -> Option<Banner> {
    if let Some(notice) = state.update.notice {
        return Some(Banner::Notice(notice));
    }
    let ready = ready_offer(state).is_some_and(|offer| !offer.skipped)
        && availability(state) == Availability::Available
        && !state.update.banner_later
        && !in_update_session(state);
    ready.then_some(Banner::Available)
}

/// The tray's update item is on (design m5b E.3): an update is ready and nothing is under way.
pub fn tray_can_update(state: &AppState) -> bool {
    ready_offer(state).is_some()
        && availability(state) == Availability::Available
        && super::navigation_enabled(state)
}

/// The next automatic check after a start (design m5b E.1): 60 s + up to 120 s when the last
/// check is 24 hours old (or never happened, or lies in the future: a clock that ran ahead,
/// RELIABILITY-8), else 24 hours after it + up to 60 minutes.
pub fn first_schedule(client: &ClientState, now: u64) -> (Duration, Duration) {
    let future = |at: Option<u64>| at.is_some_and(|at| at > now + FUTURE_TOLERANCE);
    let last = client
        .last_check
        .filter(|_| !future(client.last_check) && !future(client.last_success));
    match last {
        Some(last) if now.saturating_sub(last) < DAY => {
            (Duration::from_secs(last + DAY - now), CHECK_JITTER)
        }
        _ => (START_DELAY, START_JITTER),
    }
}

fn schedule(state: &mut AppState, after: Duration, jitter: Duration) -> Vec<Effect> {
    if !state.settings.update.auto_check || !can_check(state) {
        state.update.next_check_at = None;
        return vec![Effect::StopUpdateCheck];
    }
    state.update.next_check_at = Some(state.now_unix() + after.as_secs());
    vec![Effect::ScheduleUpdateCheck { after, jitter }]
}

/// Starts a check (automatic or "今すぐ確認").
fn start_check(state: &mut AppState, manual: bool) -> Vec<Effect> {
    if !can_check(state) || matches!(state.update.check, CheckPhase::Checking { .. }) {
        return Vec::new();
    }
    let token = state.update.next_token;
    state.update.next_token += 1;
    state.update.check = CheckPhase::Checking { token, manual };
    state.update.note = None;
    let skipped = state
        .settings
        .update
        .skipped_version
        .as_deref()
        .and_then(|text| Version::parse(text).ok());
    vec![
        Effect::Update(UpdateTask::Check {
            token,
            manual,
            skipped,
        }),
        Effect::Render,
    ]
}

/// Starts the download of the offer.
fn start_download(state: &mut AppState) -> Vec<Effect> {
    let Some(offer) = offer(state).cloned() else {
        return Vec::new();
    };
    if availability(state) != Availability::Available
        || matches!(state.update.download, DownloadPhase::Downloading { .. })
        || in_update_session(state)
    {
        return Vec::new();
    }
    let token = state.update.next_token;
    state.update.next_token += 1;
    state.update.download = DownloadPhase::Downloading {
        token,
        received: 0,
        total: offer.verified.asset.size,
    };
    state.update.download_error = None;
    state.update.session_end = None;
    state.update.note = None;
    vec![
        Effect::Update(UpdateTask::Download {
            token,
            offer: Box::new(offer),
        }),
        Effect::Render,
    ]
}

/// The 30-day notice and the expiry (design m5b E.3): once every 30 days, while automatic checks
/// are on and a check is possible.
fn stale_notice(state: &mut AppState) -> Vec<Effect> {
    let now = state.now_unix();
    if !state.settings.update.auto_check || !can_check(state) || state.update.notice.is_some() {
        return Vec::new();
    }
    let recent =
        |at: Option<u64>| at.is_some_and(|at| now.saturating_sub(at) < STALE_NOTICE_DAYS * DAY);
    if recent(state.settings.update.stale_notice_at) {
        return Vec::new();
    }
    let client = &state.update.client;
    let expired = state
        .update
        .outcome
        .as_ref()
        .map(CheckOutcome::verified)
        .filter(|verified| verified.freshness == Freshness::Expired || now > verified.expires)
        .map(|verified| verified.expires);
    let failing_since = client
        .last_success
        .or(client.last_failure.as_ref().map(|failure| failure.first_at));
    let stale = failing_since
        .is_some_and(|since| now.saturating_sub(since) >= STALE_NOTICE_DAYS * DAY)
        && client.last_failure.is_some();
    let notice = match (expired, stale) {
        (Some(expires), _) => UpdateNotice::Expired { expires },
        (None, true) => UpdateNotice::Stale {
            structural: client
                .last_failure
                .as_ref()
                .is_some_and(|failure| failure.class == ErrorClass::Structural),
        },
        (None, false) => return Vec::new(),
    };
    state.update.notice = Some(notice);
    state.settings.update.stale_notice_at = Some(now);
    vec![Effect::SaveSettings(Box::new(state.settings.clone()))]
}

/// The rollback warning (design m5b E.3): once for each ignored `issued_at`, automatic checks
/// too.
fn rollback_notice(state: &mut AppState) -> Vec<Effect> {
    let Some(note) = state.update.client.last_rollback.clone() else {
        return Vec::new();
    };
    if state.settings.update.rollback_notice_for == Some(note.issued_at) {
        return Vec::new();
    }
    state.update.notice = Some(UpdateNotice::Rollback {
        issued_at: note.issued_at,
        seen: note.seen,
    });
    state.settings.update.rollback_notice_for = Some(note.issued_at);
    vec![Effect::SaveSettings(Box::new(state.settings.clone()))]
}

/// The run IDs of the start's records that the next start must not show again, and what to show
/// now (design m5b D.13 steps 1 to 4, E.5). Pure.
pub fn result_to_show(
    settings: &crate::settings::UpdateSettings,
    machine: &MachineView,
    env: &EnvView,
    cached_version: Option<&Version>,
    installer_cached: bool,
    now: u64,
) -> (Option<ShownResult>, Option<String>) {
    let running = &env.installed;
    let inconsistent_now = env.availability == Availability::Available
        && machine.install_known
        && machine.consistent.is_none();
    let unseen =
        |result: &&UpdateResult| settings.result_seen.as_deref() != Some(result.run_id.as_str());
    // The interrupted run first (it is newer than any `LastResult`).
    let candidate = machine
        .interrupted
        .as_ref()
        .filter(unseen)
        .or(machine.last_result.as_ref().filter(unseen));
    let mut shown = None;
    let mut seen = None;
    if let Some(result) = candidate {
        seen = Some(result.run_id.as_str().to_string());
        let recent =
            (now * 1000).saturating_sub(result.finished_at.0) <= RESULT_SHOW_DAYS * DAY * 1000;
        let installed = match &result.outcome {
            UpdateOutcome::Installed => true,
            UpdateOutcome::Interrupted { .. } => {
                result.installed_version.as_deref() == Some(result.to_version.as_str())
            }
            UpdateOutcome::NotInstalled(_) | UpdateOutcome::Failed(_) => false,
        };
        let matches_now = if installed {
            running.to_string() == result.to_version
        } else {
            running.to_string() == result.from_version
                && (!matches!(
                    result.outcome,
                    UpdateOutcome::Failed(mklm_update::run::FailedReason::Inconsistent)
                ) || inconsistent_now)
        };
        let own = settings.started_run.as_deref() == Some(result.run_id.as_str());
        let closed_for_update = settings
            .closed_by_update
            .is_some_and(|closed| closed * 1000 <= result.finished_at.0);
        // Another user's failures and interruptions are not this user's business (D.13 step 3).
        if recent && matches_now && (own || installed) {
            shown = Some(ShownResult::Result {
                result: Box::new(result.clone()),
                own,
                closed_for_update,
            });
        }
    }
    if inconsistent_now {
        let version = machine
            .interrupted
            .as_ref()
            .or(machine.last_result.as_ref())
            .and_then(|result| Version::parse(&result.to_version).ok())
            .or_else(|| cached_version.cloned())
            .unwrap_or_else(|| running.clone());
        shown = Some(ShownResult::Inconsistent {
            cached: installer_cached && cached_version == Some(&version),
            version,
        });
    }
    (shown, seen)
}

/// The start's records (design m5b D.13, E.5, E.1): the result to show, the RunOnce value, the
/// cache's clean-up, the first check.
fn started(state: &mut AppState, start: UpdateStart) -> Vec<Effect> {
    let now = state.now_unix();
    let mut effects = Vec::new();
    state.update.env = Some(start.env.clone());
    state.update.machine = start.machine;
    state.update.client = start.client;
    if let Some(Ok(outcome)) = start.cached {
        state.update.outcome = Some(outcome);
    }
    let cached_version = state
        .update
        .outcome
        .as_ref()
        .map(|outcome| outcome.verified().version.clone());
    let installer_cached = ready_offer(state).is_some();
    let (shown, seen) = result_to_show(
        &state.settings.update,
        &state.update.machine,
        &start.env,
        cached_version.as_ref(),
        installer_cached,
        now,
    );
    let mut save = false;
    if let Some(run_id) = seen
        && state.settings.update.result_seen.as_deref() != Some(run_id.as_str())
    {
        state.settings.update.result_seen = Some(run_id);
        save = true;
    }
    if let Some(ShownResult::Result {
        closed_for_update: true,
        ..
    }) = &shown
    {
        state.settings.update.closed_by_update = None;
        save = true;
    }
    if let Some(shown) = shown {
        state.update.shown = Some(shown);
        if state.overlay == OverlayKind::None {
            state.overlay = OverlayKind::UpdateResult;
        }
        state.visible = true;
        effects.push(Effect::ShowWindow);
    } else if !state.elevated {
        // Nothing left to show: the after-update value has nothing to do at the next sign-in
        // (design m5b D.13 step 5).
        effects.push(Effect::AfterUpdateRunOnce(false));
    }
    // The cache keeps the installer until an update to the running version is complete and the
    // installation is in step (D.11); an offer of a newer version keeps its own.
    let running = &start.env.installed;
    let installed_here = state
        .update
        .machine
        .last_result
        .as_ref()
        .is_some_and(|result| {
            result.outcome == UpdateOutcome::Installed && result.to_version == running.to_string()
        })
        && state.update.machine.consistent.as_ref() == Some(running);
    if installed_here {
        let keep = offer(state).map(|offer| offer.verified.asset.name.clone());
        effects.push(Effect::Update(UpdateTask::Prune { keep }));
    }
    if save {
        effects.push(Effect::SaveSettings(Box::new(state.settings.clone())));
    }
    let (after, jitter) = first_schedule(&state.update.client, now);
    effects.extend(schedule(state, after, jitter));
    effects.extend(rollback_notice(state));
    effects.extend(stale_notice(state));
    // `--after-update` with nothing to show is `--tray` (E.5): the window stays hidden.
    effects.push(Effect::Render);
    effects
}

/// A check answered.
fn checked(
    state: &mut AppState,
    token: u64,
    result: Result<CheckOutcome, CheckError>,
    client: ClientState,
) -> Vec<Effect> {
    let CheckPhase::Checking {
        token: running,
        manual,
    } = state.update.check
    else {
        return Vec::new();
    };
    if running != token {
        return Vec::new();
    }
    state.update.check = CheckPhase::Idle;
    state.update.client = client;
    state.update.banner_later = false;
    let mut effects = Vec::new();
    match result {
        Ok(outcome) => {
            state.update.check_error = None;
            // A new version replaces an older download's offer.
            state.update.outcome = Some(outcome);
            if let Some(offer) = offer(state)
                && offer.downloaded.is_none()
                && !offer.skipped
                && state.settings.update.auto_check
                && availability(state) == Availability::Available
            {
                effects.extend(start_download(state));
            }
        }
        Err(error) => {
            let cancelled = matches!(
                &error,
                CheckError::Fetch(
                    mklm_update::fetch::FetchError::Cancelled
                        | mklm_update::fetch::FetchError::Transport(
                            mklm_update::fetch::TransportError::Cancelled
                        )
                )
            );
            if cancelled {
                state.update.note = manual.then_some(UpdateNote::Cancelled);
            } else {
                state.update.check_error = Some(error);
            }
        }
    }
    effects.extend(schedule(state, CHECK_INTERVAL, CHECK_JITTER));
    effects.extend(rollback_notice(state));
    effects.extend(stale_notice(state));
    effects.push(Effect::Render);
    effects
}

fn downloaded(
    state: &mut AppState,
    token: u64,
    result: Result<PathBuf, DownloadError>,
) -> Vec<Effect> {
    if !matches!(state.update.download, DownloadPhase::Downloading { token: running, .. } if running == token)
    {
        return Vec::new();
    }
    state.update.download = DownloadPhase::Idle;
    let mut effects = Vec::new();
    match result {
        Ok(path) => {
            let consistent = state.update.machine.consistent.clone();
            let installed = installed(state);
            if let Some(CheckOutcome::Available(offer)) = &mut state.update.outcome {
                offer.downloaded = Some(path);
                // Older downloads go once the installation is in step (design m5b D.11).
                if consistent.as_ref() == Some(&installed) {
                    effects.push(Effect::Update(UpdateTask::Prune {
                        keep: Some(offer.verified.asset.name.clone()),
                    }));
                }
            }
        }
        Err(DownloadError::Fetch(
            mklm_update::fetch::FetchError::Cancelled
            | mklm_update::fetch::FetchError::Transport(
                mklm_update::fetch::TransportError::Cancelled,
            ),
        )) => state.update.note = Some(UpdateNote::Cancelled),
        Err(error) => state.update.download_error = Some(error),
    }
    effects.push(Effect::Render);
    effects
}

/// "今すぐ更新" (design m5b D.2, E.4): the UAC explanation the first time, then the session.
fn update_now(state: &mut AppState) -> Vec<Effect> {
    if update_block(state).is_some() {
        return Vec::new();
    }
    if !state.elevated && !state.settings.change.uac_notice_seen {
        state.uac_origin = UacNoticeOrigin::Update;
        state.page = Page::UacNotice;
        return vec![Effect::Render];
    }
    start_session(state)
}

/// "確認画面へ進む" on the UAC explanation of an update.
pub fn uac_go(state: &mut AppState) -> Vec<Effect> {
    state.uac_origin = UacNoticeOrigin::Change;
    state.page = Page::Update;
    if update_block(state).is_some() {
        return vec![Effect::Render];
    }
    state.settings.change.uac_notice_seen = true;
    let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
    effects.extend(start_session(state));
    effects
}

/// "キャンセル" on the UAC explanation of an update: back to the update page (E.4).
pub fn uac_cancel(state: &mut AppState) -> Vec<Effect> {
    state.uac_origin = UacNoticeOrigin::Change;
    state.page = Page::Update;
    vec![Effect::Render]
}

fn start_session(state: &mut AppState) -> Vec<Effect> {
    let Some(offer) = ready_offer(state).cloned() else {
        return Vec::new();
    };
    let Some(installer) = offer.downloaded.clone() else {
        return Vec::new();
    };
    let session = state.next_session;
    state.next_session += 1;
    state.session = SessionPhase::Launching { id: session };
    state.session_purpose = Some(SessionPurpose::Update);
    state.update.session_end = None;
    state.update.note = None;
    state.update.banner_later = true;
    state.page = Page::Update;
    super::stop_identifying(state);
    vec![
        Effect::StartUpdateSession {
            session,
            offer: Box::new(offer),
            installer,
        },
        Effect::Render,
    ]
}

/// `Notice::Connected` of an update session (E.4.1: `Launching` → `Updating { Sending }`).
pub fn connected(state: &mut AppState, session: SessionId) {
    let total = ready_offer(state).map_or(0, |offer| offer.verified.asset.size);
    state.session = SessionPhase::Updating {
        id: session,
        stage: UpdateStage::Sending { sent: 0, total },
    };
}

fn session_progress(
    state: &mut AppState,
    session: SessionId,
    progress: SessionProgress,
) -> Vec<Effect> {
    let SessionPhase::Updating { id, stage } = &mut state.session else {
        return Vec::new();
    };
    if *id != session {
        return Vec::new();
    }
    match (progress, *stage) {
        (SessionProgress::Sent { bytes, total }, UpdateStage::Sending { sent, total: was }) => {
            let percent = |bytes: u64, total: u64| bytes * 100 / total.max(1);
            let changed = percent(bytes, total) != percent(sent, was);
            *stage = UpdateStage::Sending { sent: bytes, total };
            if changed {
                vec![Effect::Render]
            } else {
                Vec::new()
            }
        }
        (SessionProgress::StartingRunner, UpdateStage::Sending { .. }) => {
            *stage = UpdateStage::StartingRunner;
            vec![Effect::Render]
        }
        _ => Vec::new(),
    }
}

/// The end of an update session (E.4, E.4.1).
fn session_ended(state: &mut AppState, session: SessionId, end: UpdateSessionEnd) -> Vec<Effect> {
    if state.session.id() != Some(session) || !in_update_session(state) {
        return Vec::new();
    }
    if let UpdateSessionEnd::Stage(StageEnd::HandedOff { run_id, .. }) = &end {
        return handed_off(state, session, run_id.clone());
    }
    state.session = SessionPhase::Idle;
    state.session_purpose = None;
    let cancelled = matches!(
        end,
        UpdateSessionEnd::NotLaunched(LaunchError::Declined)
            | UpdateSessionEnd::Stage(StageEnd::Cancelled)
    );
    if cancelled {
        state.update.note = Some(UpdateNote::Cancelled);
    } else {
        // The file changed since it was checked: it is downloaded again.
        if matches!(
            end,
            UpdateSessionEnd::Stage(StageEnd::SourceChanged)
                | UpdateSessionEnd::Stage(StageEnd::Refused(
                    UpdateRefusal::InstallerHashMismatch
                        | UpdateRefusal::InstallerSizeMismatch { .. }
                ))
        ) && let Some(CheckOutcome::Available(offer)) = &mut state.update.outcome
        {
            offer.downloaded = None;
        }
        state.update.session_end = Some(end);
    }
    let mut effects = vec![Effect::Update(UpdateTask::ReadStatus), Effect::Render];
    if state.quit_pending {
        effects.push(Effect::Quit);
    }
    effects
}

/// `HandedOff` (or the runner seen taking over, RELIABILITY-5): the run is this user's, the
/// after-update value is registered, the overlay says what happens, then MKLM quits (E.4).
fn handed_off(state: &mut AppState, session: SessionId, run_id: String) -> Vec<Effect> {
    state.session = SessionPhase::Updating {
        id: session,
        stage: UpdateStage::HandedOff,
    };
    state.settings.update.started_run = Some(run_id);
    state.overlay = OverlayKind::UpdateHandOff;
    state.visible = true;
    let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
    if !state.elevated {
        effects.push(Effect::AfterUpdateRunOnce(true));
    }
    effects.extend([
        Effect::HandOffTimer(HANDOFF_OVERLAY_MAX),
        Effect::ShowWindow,
        Effect::Render,
    ]);
    // A quit asked for while the runner started happens now (E.4.1).
    if state.quit_pending {
        effects.push(Effect::Quit);
    }
    effects
}

/// The single-instance pipe's `quit-if-idle` (design m5b E.4.1; RELIABILITY-1, OPS-UX-TEST-4):
/// only when nothing is going on — no session, no pending quit, no question — MKLM registers
/// the after-update value (unelevated), notes the time and quits (`ok`). Otherwise nothing at all
/// changes (`busy`). A command that reached the UI thread more than 4 s after the pipe thread
/// got it is ignored: nobody waits for its answer any more.
pub fn quit_if_idle(state: &mut AppState, received: Instant) -> (Vec<Effect>, InstanceReply) {
    let late = state
        .now
        .is_some_and(|now| now.saturating_duration_since(received) > QUIT_IF_IDLE_WAIT);
    if late {
        return (Vec::new(), InstanceReply::Busy);
    }
    let questioning = matches!(
        state.overlay,
        OverlayKind::QuitConfirm
            | OverlayKind::Reconnect
            | OverlayKind::RecoveryConfirm
            | OverlayKind::Countdown
            | OverlayKind::Progress
            | OverlayKind::UpdateHandOff
    );
    if state.session != SessionPhase::Idle || state.quit_pending || questioning {
        return (Vec::new(), InstanceReply::Busy);
    }
    let mut effects = Vec::new();
    if !state.elevated {
        effects.push(Effect::AfterUpdateRunOnce(true));
    }
    state.settings.update.closed_by_update = Some(state.now_unix());
    effects.push(Effect::SaveSettings(Box::new(state.settings.clone())));
    effects.push(Effect::Quit);
    (effects, InstanceReply::Ok)
}

/// What MKLM does at start while an update runs (design m5b D.13 step 1): `None` to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartGate {
    /// Quit at once, without a window and without becoming the instance; the runner starts this
    /// session's MKLM again (the RunOnce value is left alone).
    QuitQuietly,
    /// Another session's update: register the after-update value (unelevated), note the time in
    /// `closed_by_update`, then quit (FIX-VERIFICATION-9).
    QuitAndRemember,
}

/// The start-up decision (pure): an update past `ready` whose runner or installer lives keeps
/// MKLM from starting, since its window would lock `mklm.exe`.
pub fn start_gate(
    run: &RunView,
    caller_session: Option<u32>,
    my_session: Option<u32>,
) -> Option<StartGate> {
    let running = matches!(
        run,
        RunView::InProgress { phase, .. } if !matches!(phase, RunPhase::Staging | RunPhase::Staged)
    );
    if !running {
        return None;
    }
    Some(match (caller_session, my_session) {
        (Some(caller), Some(mine)) if caller == mine => StartGate::QuitQuietly,
        _ => StartGate::QuitAndRemember,
    })
}

/// What a start that [`start_gate`] stopped does before it ends (pure; `app.rs` carries out
/// `AfterUpdateRunOnce` and `SaveSettings` and nothing else). `QuitQuietly`: nothing, and the
/// user's settings are not even read (the runner starts this session's MKLM again).
/// `QuitAndRemember`: the after-update value unless MKLM is elevated (`elevated` is true also when
/// that could not be told), then `closed_by_update = now_unix` in the user's settings, saved —
/// no save when `settings` has none to give (no settings folder).
pub fn start_gate_effects(
    gate: StartGate,
    elevated: bool,
    now_unix: u64,
    settings: impl FnOnce() -> Option<crate::settings::Settings>,
) -> Vec<Effect> {
    if gate == StartGate::QuitQuietly {
        return Vec::new();
    }
    let mut effects = Vec::new();
    if !elevated {
        effects.push(Effect::AfterUpdateRunOnce(true));
    }
    if let Some(mut settings) = settings() {
        settings.update.closed_by_update = Some(now_unix);
        effects.push(Effect::SaveSettings(Box::new(settings)));
    }
    effects
}

/// Handles an [`UpdateMsg`].
pub fn handle(state: &mut AppState, msg: UpdateMsg) -> Vec<Effect> {
    match msg {
        UpdateMsg::Started(start) => started(state, *start),
        UpdateMsg::StatusRead(machine) => {
            state.update.machine = *machine;
            vec![Effect::Render]
        }
        UpdateMsg::CheckDue => {
            state.update.next_check_at = None;
            if !state.settings.update.auto_check {
                return Vec::new();
            }
            if in_update_session(state) {
                return schedule(state, BUSY_RETRY, Duration::ZERO);
            }
            start_check(state, false)
        }
        UpdateMsg::CheckNow => start_check(state, true),
        UpdateMsg::Cancel => {
            if in_update_session(state) {
                // Honoured until every byte is sent (E.2); the session's end says the rest.
                return match state.session {
                    SessionPhase::Launching { .. }
                    | SessionPhase::Updating {
                        stage: UpdateStage::Sending { .. },
                        ..
                    } => vec![Effect::CancelSession],
                    _ => Vec::new(),
                };
            }
            let busy = matches!(state.update.check, CheckPhase::Checking { .. })
                || matches!(state.update.download, DownloadPhase::Downloading { .. });
            if busy {
                vec![Effect::CancelUpdateTask]
            } else {
                Vec::new()
            }
        }
        UpdateMsg::Checked {
            token,
            result,
            client,
        } => checked(state, token, *result, *client),
        UpdateMsg::Download => start_download(state),
        UpdateMsg::DownloadProgress {
            token,
            received,
            total,
        } => {
            let DownloadPhase::Downloading {
                token: running,
                received: was,
                ..
            } = state.update.download
            else {
                return Vec::new();
            };
            if running != token {
                return Vec::new();
            }
            state.update.download = DownloadPhase::Downloading {
                token,
                received,
                total,
            };
            let percent = |bytes: u64| bytes * 100 / total.max(1);
            if percent(received) != percent(was) {
                vec![Effect::Render]
            } else {
                Vec::new()
            }
        }
        UpdateMsg::Downloaded { token, result } => downloaded(state, token, *result),
        UpdateMsg::Skip => {
            let Some(version) = offer(state).map(|offer| offer.verified.version.to_string()) else {
                return Vec::new();
            };
            state.settings.update.skipped_version = Some(version);
            if let Some(CheckOutcome::Available(offer)) = &mut state.update.outcome {
                offer.skipped = true;
            }
            vec![
                Effect::SaveSettings(Box::new(state.settings.clone())),
                Effect::Render,
            ]
        }
        UpdateMsg::Later => {
            state.update.banner_later = true;
            let mut effects = super::update(state, super::AppMsg::Navigate(Page::Main));
            if !effects.contains(&Effect::Render) {
                effects.push(Effect::Render);
            }
            effects
        }
        UpdateMsg::UpdateNow => update_now(state),
        UpdateMsg::OpenPage => {
            state.visible = true;
            let mut effects = vec![Effect::ShowWindow];
            if super::navigation_enabled(state) || state.page == Page::Update {
                state.update.notice = None;
                effects.extend(super::update(state, super::AppMsg::Navigate(Page::Update)));
            } else {
                effects.push(Effect::Render);
            }
            effects
        }
        UpdateMsg::OpenReleaseNotes | UpdateMsg::OpenReleasePage => {
            let version = release_version(state);
            vec![Effect::Update(UpdateTask::OpenReleasePage(
                version.to_string(),
            ))]
        }
        UpdateMsg::CopyDetails => {
            let text = crate::vm::update::details(state);
            if text.is_empty() {
                Vec::new()
            } else {
                vec![Effect::CopyText(text)]
            }
        }
        UpdateMsg::RunInstaller => {
            if in_update_session(state) || state.session != SessionPhase::Idle {
                return Vec::new();
            }
            vec![Effect::Update(UpdateTask::RunInstaller)]
        }
        UpdateMsg::InstallerRan(result) => {
            state.update.note = match result {
                Ok(()) => None,
                Err(InstallerRunError::Cancelled) => Some(UpdateNote::Cancelled),
                Err(InstallerRunError::Failed(detail)) => Some(UpdateNote::InstallerFailed(detail)),
            };
            vec![Effect::Render]
        }
        UpdateMsg::AutoCheck(on) => {
            if state.settings.update.auto_check == on {
                return Vec::new();
            }
            state.settings.update.auto_check = on;
            let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
            if on {
                let (after, jitter) = first_schedule(&state.update.client, state.now_unix());
                effects.extend(schedule(state, after, jitter));
            } else {
                state.update.next_check_at = None;
                effects.push(Effect::StopUpdateCheck);
            }
            effects.push(Effect::Render);
            effects
        }
        UpdateMsg::Resumed => match state.update.next_check_at {
            Some(due) if state.now_unix() >= due => schedule(state, RESUME_DELAY, Duration::ZERO),
            _ => Vec::new(),
        },
        UpdateMsg::SessionProgress { session, progress } => {
            session_progress(state, session, progress)
        }
        UpdateMsg::SessionEnded { session, end } => session_ended(state, session, *end),
        UpdateMsg::HandOffClosed => {
            if state.overlay != OverlayKind::UpdateHandOff {
                return Vec::new();
            }
            vec![Effect::Quit]
        }
        UpdateMsg::ResultClosed => {
            if state.overlay != OverlayKind::UpdateResult {
                return Vec::new();
            }
            state.overlay = OverlayKind::None;
            state.update.shown = None;
            vec![Effect::Render]
        }
        UpdateMsg::Review => {
            state.overlay = OverlayKind::None;
            let mut effects = super::update(state, super::AppMsg::Navigate(Page::Main));
            effects.extend(super::update(
                state,
                super::AppMsg::BannerAction {
                    at: state.now.unwrap_or_else(Instant::now),
                },
            ));
            effects
        }
        UpdateMsg::RestartPage => {
            state.overlay = OverlayKind::None;
            super::update(state, super::AppMsg::Navigate(Page::Restart))
        }
    }
}

/// The version the release notes and the release page are about: the offered one, a result's,
/// else the installed one.
pub fn release_version(state: &AppState) -> Version {
    if let Some(ShownResult::Result { result, .. }) = &state.update.shown
        && let Ok(version) = Version::parse(&result.to_version)
    {
        return version;
    }
    if let Some(ShownResult::Inconsistent { version, .. }) = &state.update.shown {
        return version.clone();
    }
    state
        .update
        .outcome
        .as_ref()
        .map(|outcome| outcome.verified().version.clone())
        .unwrap_or_else(|| installed(state))
}

#[cfg(test)]
pub(crate) mod tests;
