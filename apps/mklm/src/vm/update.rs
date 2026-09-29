//! Updates as the screens show them (design m5b E.2 to E.7, D.13): the update page, the main
//! screen's banner, the settings section, the tray, the hand-off overlay and the result overlay.
//!
//! Every typed value of design m5b E.6 — `UpdateRefusal`, `FetchError`, `TransportError`,
//! `CheckError`, `DownloadError`, `StageEnd`, `UpdateOutcome`, `NotInstalledReason`,
//! `FailedReason`, `InstallerExit`, `Availability`, `Freshness`, `RunPhase`, `ProgramKind` — is
//! mapped here by an exhaustive `match` onto a message ([`Msg`], worded by `i18n::update`) and the
//! next steps the table names ([`UpdateAction`]). English diagnostics go to the technical details
//! and "詳細をコピー" only.

use mklm_client::orchestrator::LaunchError;
use mklm_client::update::cache::ErrorClass;
use mklm_client::update::check::{CheckError, CheckOutcome};
use mklm_client::update::classify;
use mklm_client::update::download::DownloadError;
use mklm_client::update::env::Availability;
use mklm_client::update::stage::StageEnd;
use mklm_update::fetch::{FetchError, TransportError};
use mklm_update::run::{
    FailedReason, InstallerExit, NotInstalledReason, UpdateOutcome, UpdateResult,
};
use mklm_update::{Freshness, UpdateRefusal, Version};

use super::{SnapshotText, Tone};
use crate::i18n::Lang;
use crate::i18n::update::{self as text, Button, Msg, Params};
use crate::state::update::{
    self as rules, Banner, CheckPhase, DownloadPhase, PageState, ShownResult, UpdateBlock,
    UpdateNote, UpdateNotice, UpdateSessionEnd,
};
use crate::state::{AppState, SessionPhase, UpdateStage};

/// A button of the update page, a banner, the settings section or the result (the index is what
/// the Slint callback passes back).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateAction {
    CheckNow,
    Cancel,
    Download,
    Skip,
    Later,
    UpdateNow,
    ReleaseNotes,
    ReleasePage,
    CopyDetails,
    RunInstaller,
    Review,
    Restart,
    /// Opens the update page.
    Details,
    Close,
}

impl UpdateAction {
    const ALL: [UpdateAction; 14] = [
        UpdateAction::CheckNow,
        UpdateAction::Cancel,
        UpdateAction::Download,
        UpdateAction::Skip,
        UpdateAction::Later,
        UpdateAction::UpdateNow,
        UpdateAction::ReleaseNotes,
        UpdateAction::ReleasePage,
        UpdateAction::CopyDetails,
        UpdateAction::RunInstaller,
        UpdateAction::Review,
        UpdateAction::Restart,
        UpdateAction::Details,
        UpdateAction::Close,
    ];

    pub fn index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|action| *action == self)
            .map_or(-1, |index| index as i32)
    }

    pub fn from_index(index: i32) -> Option<UpdateAction> {
        usize::try_from(index)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
    }

    /// What pressing it does.
    pub fn message(self) -> crate::state::UpdateMsg {
        use crate::state::UpdateMsg;
        match self {
            UpdateAction::CheckNow => UpdateMsg::CheckNow,
            UpdateAction::Cancel => UpdateMsg::Cancel,
            UpdateAction::Download => UpdateMsg::Download,
            UpdateAction::Skip => UpdateMsg::Skip,
            UpdateAction::Later => UpdateMsg::Later,
            UpdateAction::UpdateNow => UpdateMsg::UpdateNow,
            UpdateAction::ReleaseNotes => UpdateMsg::OpenReleaseNotes,
            UpdateAction::ReleasePage => UpdateMsg::OpenReleasePage,
            UpdateAction::CopyDetails => UpdateMsg::CopyDetails,
            UpdateAction::RunInstaller => UpdateMsg::RunInstaller,
            UpdateAction::Review => UpdateMsg::Review,
            UpdateAction::Restart => UpdateMsg::RestartPage,
            UpdateAction::Details => UpdateMsg::OpenPage,
            UpdateAction::Close => UpdateMsg::ResultClosed,
        }
    }
}

/// A button as shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionView {
    pub action: UpdateAction,
    pub text: String,
    /// The accessible name (design m5b E.7).
    pub label: String,
    pub primary: bool,
    pub enabled: bool,
}

/// A local date and time (converted by `app.rs` with `mklm_win::time`; tests use a fixed zone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalMinute {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Unix seconds → local time.
pub type Clock<'a> = &'a dyn Fn(u64) -> LocalMinute;

/// A clock `offset_minutes` east of UTC (tests; `app.rs` falls back to UTC when the zone cannot
/// be read).
pub fn fixed_offset(offset_minutes: i64, unix: u64) -> LocalMinute {
    let local = unix as i64 + offset_minutes * 60;
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    LocalMinute {
        year: yoe + era * 400 + i64::from(month <= 2),
        month,
        day,
        hour: (seconds / 3600) as u32,
        minute: ((seconds % 3600) / 60) as u32,
    }
}

/// "2026/10/15" / "2026-10-15".
pub fn date(unix: u64, clock: Clock<'_>, lang: Lang) -> String {
    let t = clock(unix);
    match lang {
        Lang::Ja => format!("{}/{:02}/{:02}", t.year, t.month, t.day),
        Lang::En => format!("{}-{:02}-{:02}", t.year, t.month, t.day),
    }
}

/// "2026/10/16 09:12" / "2026-10-16 09:12".
pub fn date_time(unix: u64, clock: Clock<'_>, lang: Lang) -> String {
    let t = clock(unix);
    format!("{} {:02}:{:02}", date(unix, clock, lang), t.hour, t.minute)
}

/// A message with its next steps (design m5b E.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explained {
    pub msg: Msg,
    pub actions: Vec<UpdateAction>,
}

fn explained(msg: Msg, actions: &[UpdateAction]) -> Explained {
    Explained {
        msg,
        actions: actions.to_vec(),
    }
}

/// `FetchError` (and its `TransportError`), for a check or a download (E.6).
pub fn explain_fetch(error: &FetchError, download: bool, not_found_days: u64) -> Explained {
    use UpdateAction::{CheckNow, Download, ReleasePage};
    let retry = if download { Download } else { CheckNow };
    let network = if download { Msg::NetDl } else { Msg::Net };
    match error {
        FetchError::Transport(transport) => match transport {
            TransportError::Timeout
            | TransportError::NameNotResolved
            | TransportError::CannotConnect
            | TransportError::Other { .. } => explained(network, &[retry]),
            TransportError::ProxyAuthRequired => explained(Msg::ProxyAuth, &[ReleasePage]),
            TransportError::Tls => explained(Msg::Tls, &[retry]),
            TransportError::Cancelled => explained(Msg::Cancelled, &[]),
        },
        FetchError::DeadlineExceeded => explained(network, &[retry]),
        FetchError::Cancelled => explained(Msg::Cancelled, &[]),
        FetchError::NotFound
            if not_found_days >= mklm_update::run::timing::NOT_FOUND_STRUCTURAL_DAYS =>
        {
            explained(Msg::NotFoundLong, &[ReleasePage])
        }
        FetchError::NotFound => explained(Msg::NotFound, &[retry]),
        FetchError::RateLimited { .. } | FetchError::HttpStatus { .. } => {
            explained(Msg::GhTemp, &[retry])
        }
        FetchError::TooManyRedirects
        | FetchError::RedirectNotAllowed { .. }
        | FetchError::MissingLocation
        | FetchError::UnexpectedEncoding => explained(Msg::GhChanged, &[ReleasePage]),
        FetchError::TooLarge { .. } if download => explained(Msg::DownloadMismatch, &[Download]),
        FetchError::TooLarge { .. } => explained(Msg::Format, &[ReleasePage]),
        FetchError::SizeMismatch { .. } | FetchError::HashMismatch => {
            explained(Msg::DownloadMismatch, &[Download])
        }
        FetchError::Sink { .. } => explained(Msg::Cache, &[Download]),
    }
}

/// `UpdateRefusal` (E.6).
pub fn explain_refusal(refusal: &UpdateRefusal) -> Explained {
    use UpdateAction::{CheckNow, CopyDetails, Download, ReleasePage, Restart, Review, UpdateNow};
    match refusal {
        UpdateRefusal::SignatureTooLarge { .. }
        | UpdateRefusal::SignatureMalformed
        | UpdateRefusal::WrongTrustedComment
        | UpdateRefusal::UnknownKey { .. }
        | UpdateRefusal::BadSignature
        | UpdateRefusal::RevokedKey { .. }
        | UpdateRefusal::SignerNotListed { .. }
        | UpdateRefusal::IllegalRevocation { .. } => explained(Msg::Sig, &[ReleasePage]),
        UpdateRefusal::ManifestTooLarge { .. }
        | UpdateRefusal::ManifestMalformed { .. }
        | UpdateRefusal::UnsupportedSchema { .. }
        | UpdateRefusal::WrongProduct
        | UpdateRefusal::WrongChannel
        | UpdateRefusal::BadVersion { .. }
        | UpdateRefusal::BadTimestamps
        | UpdateRefusal::AssetMalformed { .. }
        | UpdateRefusal::NoAssetForArch { .. } => explained(Msg::Format, &[ReleasePage]),
        UpdateRefusal::TagMismatch { .. } => explained(Msg::Race, &[CheckNow]),
        UpdateRefusal::Rollback { .. } => explained(Msg::Rollback, &[ReleasePage]),
        UpdateRefusal::NotNewer { .. } => explained(Msg::NotNewer, &[CheckNow]),
        UpdateRefusal::ManualUpdateRequired { .. } => explained(Msg::Manual, &[ReleasePage]),
        UpdateRefusal::NotConfigured => explained(Msg::NotConfigured, &[]),
        UpdateRefusal::NotInstalledCopy => explained(Msg::NotInstalledCopy, &[CheckNow]),
        UpdateRefusal::OperationOpen {
            waiting_for_reboot: false,
        }
        | UpdateRefusal::RecoveryNeeded => explained(Msg::OpOpen, &[Review]),
        UpdateRefusal::OperationOpen {
            waiting_for_reboot: true,
        } => explained(Msg::OpReboot, &[Restart]),
        UpdateRefusal::Busy | UpdateRefusal::UpdateInProgress => explained(Msg::Busy, &[UpdateNow]),
        UpdateRefusal::DiskFull { .. } => explained(Msg::DiskFull, &[UpdateNow]),
        UpdateRefusal::InstallerSizeMismatch { .. }
        | UpdateRefusal::InstallerHashMismatch
        | UpdateRefusal::ChunkMalformed
        | UpdateRefusal::ChunkOutOfOrder { .. } => explained(Msg::StageMismatch, &[Download]),
        UpdateRefusal::HandOffFailed { .. }
        | UpdateRefusal::Storage { .. }
        | UpdateRefusal::Internal { .. }
        | UpdateRefusal::JournalUnreadable
        | UpdateRefusal::CallerLeft => explained(Msg::PrepareFailed, &[CopyDetails]),
    }
}

/// `Availability` (E.6); `None` for `Available`.
pub fn explain_availability(availability: &Availability) -> Option<Explained> {
    Some(match availability {
        Availability::Available => return None,
        Availability::NotConfigured => explained(Msg::NotConfigured, &[]),
        Availability::NotInstalledCopy { .. } => {
            explained(Msg::NotInstalledCopy, &[UpdateAction::CheckNow])
        }
        Availability::Unknown { .. } => explained(Msg::EnvUnknown, &[UpdateAction::CopyDetails]),
    })
}

/// `CheckError` (E.6): what is inside it.
pub fn explain_check(error: &CheckError, not_found_days: u64) -> Explained {
    match error {
        CheckError::Unavailable(availability) => explain_availability(availability)
            .unwrap_or_else(|| explained(Msg::PrepareFailed, &[UpdateAction::CopyDetails])),
        CheckError::Fetch(error) => explain_fetch(error, false, not_found_days),
        CheckError::Refused(refusal) => explain_refusal(refusal),
        CheckError::Cache(_) => explained(Msg::Cache, &[UpdateAction::Download]),
    }
}

/// `DownloadError` (E.6).
pub fn explain_download(error: &DownloadError) -> Explained {
    match error {
        DownloadError::Fetch(error) => explain_fetch(error, true, 0),
        DownloadError::Cache(_) => explained(Msg::Cache, &[UpdateAction::Download]),
    }
}

/// `StageEnd` (E.6).
pub fn explain_stage(end: &StageEnd) -> Explained {
    match end {
        StageEnd::HandedOff { .. } => explained(Msg::Handoff, &[UpdateAction::Close]),
        StageEnd::Refused(refusal) => explain_refusal(refusal),
        StageEnd::Cancelled => explained(Msg::Cancelled, &[]),
        StageEnd::SourceChanged => explained(Msg::DownloadMismatch, &[UpdateAction::Download]),
        StageEnd::Lost(_) | StageEnd::Unresponsive | StageEnd::Protocol(_) => {
            explained(Msg::PrepareFailed, &[UpdateAction::CopyDetails])
        }
    }
}

/// How an update session ended without a hand-off.
pub fn explain_session_end(end: &UpdateSessionEnd) -> Explained {
    match end {
        UpdateSessionEnd::NotLaunched(LaunchError::Declined) => explained(Msg::Cancelled, &[]),
        UpdateSessionEnd::NotLaunched(LaunchError::Failed { .. }) => {
            explained(Msg::PrepareFailed, &[UpdateAction::CopyDetails])
        }
        UpdateSessionEnd::NotReady(error) => match error {
            // The cached file is no longer what was checked: download it again.
            CheckError::Cache(_) => explained(Msg::DownloadMismatch, &[UpdateAction::Download]),
            error => explain_check(error, 0),
        },
        UpdateSessionEnd::Stage(end) => explain_stage(end),
    }
}

/// `InstallerExit` inside `NotInstalled(InstallerRefused)` (E.6).
pub fn explain_installer_exit(exit: InstallerExit) -> Explained {
    use UpdateAction::{CopyDetails, ReleasePage, UpdateNow};
    match exit {
        InstallerExit::HelperRunning | InstallerExit::CliRunning | InstallerExit::GuiRunning => {
            explained(Msg::ProgramsRunning, &[UpdateNow, CopyDetails])
        }
        InstallerExit::FilesInUse => explained(Msg::FilesInUse, &[UpdateNow, CopyDetails]),
        InstallerExit::OsTooOld | InstallerExit::WrongArch => {
            explained(Msg::InstallerEnv, &[ReleasePage])
        }
        InstallerExit::FileWrite => explained(Msg::FileWrite, &[UpdateNow]),
        InstallerExit::Success
        | InstallerExit::UserCancelled
        | InstallerExit::ScriptAborted
        | InstallerExit::Other(_) => explained(Msg::NotInstalled, &[UpdateNow, CopyDetails]),
    }
}

/// `NotInstalledReason` (E.6).
pub fn explain_not_installed(reason: &NotInstalledReason) -> Explained {
    use UpdateAction::{CopyDetails, UpdateNow};
    match reason {
        NotInstalledReason::Refused(refusal) => explain_refusal(refusal),
        NotInstalledReason::CallerDidNotExit | NotInstalledReason::ProgramsStillRunning { .. } => {
            explained(Msg::ProgramsRunning, &[UpdateNow, CopyDetails])
        }
        NotInstalledReason::InstanceBusy { .. } => {
            explained(Msg::InstanceBusy, &[UpdateNow, CopyDetails])
        }
        NotInstalledReason::FilesInUse { .. } => {
            explained(Msg::FilesInUse, &[UpdateNow, CopyDetails])
        }
        NotInstalledReason::DiskFull { .. } => explained(Msg::DiskFull, &[UpdateNow]),
        NotInstalledReason::SessionEnding => explained(Msg::SessionEnding, &[UpdateNow]),
        NotInstalledReason::InstalledVersionChanged { .. } => explained(Msg::VersionChanged, &[]),
        NotInstalledReason::InstallerNotStarted { code: 225 | 226 } => {
            explained(Msg::AvBlocked, &[CopyDetails])
        }
        NotInstalledReason::InstallerNotStarted { .. } => {
            explained(Msg::InstallerNotStarted, &[UpdateNow, CopyDetails])
        }
        NotInstalledReason::InstallerRefused { exit } => explain_installer_exit(*exit),
        NotInstalledReason::InstallerExit { .. } => {
            explained(Msg::NotInstalled, &[UpdateNow, CopyDetails])
        }
    }
}

/// `FailedReason` (E.6).
pub fn explain_failed(reason: &FailedReason) -> Explained {
    let steps = [
        UpdateAction::RunInstaller,
        UpdateAction::ReleasePage,
        UpdateAction::CopyDetails,
    ];
    match reason {
        FailedReason::Inconsistent | FailedReason::UnexpectedVersion { .. } => {
            explained(Msg::Inconsistent, &steps)
        }
        FailedReason::InstallerTimedOut => explained(Msg::Timeout, &steps),
    }
}

/// `UpdateOutcome` for this user (`own`) or another (E.6, D.13 table).
pub fn explain_outcome(result: &UpdateResult, own: bool) -> Explained {
    match &result.outcome {
        UpdateOutcome::Installed if own => explained(Msg::Installed, &[UpdateAction::ReleaseNotes]),
        UpdateOutcome::Installed => explained(Msg::InstalledOther, &[UpdateAction::ReleaseNotes]),
        UpdateOutcome::NotInstalled(reason) => explain_not_installed(reason),
        UpdateOutcome::Failed(reason) => explain_failed(reason),
        UpdateOutcome::Interrupted { phase } => {
            let new_in_place =
                result.installed_version.as_deref() == Some(result.to_version.as_str());
            let installing = matches!(
                phase,
                mklm_update::run::RunPhase::Installing | mklm_update::run::RunPhase::Finishing
            );
            match (new_in_place, installing, own) {
                (true, _, true) => {
                    explained(Msg::InterruptedInstalled, &[UpdateAction::ReleaseNotes])
                }
                (true, _, false) => explained(Msg::InstalledOther, &[UpdateAction::ReleaseNotes]),
                (false, true, _) => explained(Msg::InterruptedKept, &[UpdateAction::UpdateNow]),
                (false, false, _) => explained(Msg::InterruptedNothing, &[UpdateAction::UpdateNow]),
            }
        }
    }
}

/// `Freshness` (E.6): the expiry line, `None` while fresh.
pub fn explain_freshness(freshness: Freshness) -> Option<Explained> {
    match freshness {
        Freshness::Fresh => None,
        Freshness::Expired => Some(explained(Msg::Expired, &[UpdateAction::ReleasePage])),
    }
}

/// The message's text with the names of this state.
pub fn words(msg: Msg, params: &Params, lang: Lang) -> String {
    let text = text::text(msg, params, lang);
    // "Not installed" and "failed" always say the keyboards did not change (E.6).
    let unchanged = matches!(
        msg,
        Msg::InstanceBusy
            | Msg::ProgramsRunning
            | Msg::FilesInUse
            | Msg::SessionEnding
            | Msg::AvBlocked
            | Msg::InstallerEnv
            | Msg::FileWrite
            | Msg::Inconsistent
            | Msg::Timeout
            | Msg::VersionChanged
            | Msg::InterruptedNothing
            | Msg::InterruptedKept
            | Msg::DiskFull
            | Msg::NotInstalled
            | Msg::InstallerNotStarted
    );
    if unchanged {
        text::with_keyboards_unchanged(text, lang)
    } else {
        text
    }
}

/// The names the messages of `state` use.
pub fn params(state: &AppState, lang: Lang) -> Params {
    let installed = rules::installed(state);
    let offered = rules::release_version(state);
    let path = match rules::availability(state) {
        Availability::NotInstalledCopy { exe_dir, .. } => exe_dir.display().to_string(),
        _ => String::new(),
    };
    let arch = mklm_update::Arch::of_this_build();
    let (needed_mb, programs, reason) = failure_names(state, lang);
    Params {
        installed: installed.to_string(),
        offered: offered.to_string(),
        path,
        installer: mklm_update::installer_name(&offered, arch),
        needed_mb,
        programs,
        reason,
        // Set where a date is shown (the expiry, with the page's clock).
        date: String::new(),
        state_now: state_now(state, lang),
    }
}

/// The disk space, programs and reasons a failure names.
fn failure_names(state: &AppState, lang: Lang) -> (u64, String, String) {
    let mut needed = 0;
    let mut programs = String::new();
    let mut reason = String::new();
    let refusal = |end: &UpdateSessionEnd| match end {
        UpdateSessionEnd::Stage(StageEnd::Refused(refusal)) => Some(refusal.clone()),
        _ => None,
    };
    let mb = |needed: u64, available: u64| needed.saturating_sub(available).div_ceil(1_048_576);
    if let Some(UpdateRefusal::DiskFull {
        needed: n,
        available,
    }) = state.update.session_end.as_ref().and_then(refusal)
    {
        needed = mb(n, available);
    }
    if let Some(ShownResult::Result { result, .. }) = &state.update.shown {
        match &result.outcome {
            UpdateOutcome::NotInstalled(NotInstalledReason::DiskFull {
                needed: n,
                available,
            }) => {
                needed = mb(*n, *available);
            }
            UpdateOutcome::NotInstalled(
                NotInstalledReason::ProgramsStillRunning {
                    programs: kinds, ..
                }
                | NotInstalledReason::FilesInUse {
                    programs: kinds, ..
                },
            ) => {
                let names: Vec<&str> = kinds.iter().map(|kind| text::program(*kind)).collect();
                programs = text::list(&names, lang);
            }
            UpdateOutcome::NotInstalled(NotInstalledReason::InstallerRefused { exit }) => {
                match exit {
                    InstallerExit::HelperRunning => programs = "mklm-helper".into(),
                    InstallerExit::CliRunning => programs = "mklm-cli".into(),
                    InstallerExit::GuiRunning => programs = "MKLM".into(),
                    InstallerExit::OsTooOld => reason = text::installer_env_reason(true, lang),
                    InstallerExit::WrongArch => reason = text::installer_env_reason(false, lang),
                    _ => {}
                }
            }
            UpdateOutcome::NotInstalled(NotInstalledReason::CallerDidNotExit) => {
                programs = "MKLM".into();
            }
            _ => {}
        }
    }
    (needed, programs, reason)
}

/// "今の MKLM は 0.2.0 です。", or the out-of-step text (design m5b E.6 `upd-timeout`).
fn state_now(state: &AppState, lang: Lang) -> String {
    if rules::inconsistent(state) {
        let params = Params {
            installer: mklm_update::installer_name(
                &rules::release_version(state),
                mklm_update::Arch::of_this_build(),
            ),
            ..Params::default()
        };
        text::text(Msg::Inconsistent, &params, lang)
    } else {
        text::version_now(&rules::installed(state).to_string(), lang)
    }
}

fn action(action: UpdateAction, version: &Version, lang: Lang) -> ActionView {
    let button = match action {
        UpdateAction::CheckNow => Button::CheckNow,
        UpdateAction::Cancel => Button::Cancel,
        UpdateAction::Download => Button::Download,
        UpdateAction::Skip => Button::Skip,
        UpdateAction::Later => Button::Later,
        UpdateAction::UpdateNow => Button::UpdateNow,
        UpdateAction::ReleaseNotes => Button::ReleaseNotes,
        UpdateAction::ReleasePage => Button::ReleasePage,
        UpdateAction::CopyDetails => Button::CopyDetails,
        UpdateAction::RunInstaller => Button::RunInstaller,
        UpdateAction::Review => Button::Review,
        UpdateAction::Restart => Button::Restart,
        UpdateAction::Details => Button::Details,
        UpdateAction::Close => Button::Close,
    };
    let version = version.to_string();
    ActionView {
        action,
        text: text::button(button, lang),
        label: text::button_label(button, &version, lang),
        primary: false,
        enabled: true,
    }
}

/// The page (design m5b E.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdatePageView {
    pub title: String,
    pub status: String,
    pub tone: Tone,
    /// "新しい版があります: 0.2.1（今の版: 0.2.0）".
    pub version_line: String,
    pub published: String,
    /// The download's state ("ダウンロード: 完了（…）"), or the session's step.
    pub progress_text: String,
    /// 0 to 100 while a download or a hand-over runs; `None` otherwise.
    pub progress: Option<u32>,
    /// The progress bar's accessible name ("ダウンロード: 45 %").
    pub progress_label: String,
    /// The same every 25 %, read out politely (design m5b E.7).
    pub progress_announcement: String,
    /// Before "今すぐ更新": the UAC prompt and what to check on it (SECURITY-14).
    pub explanation: String,
    /// Expiry, 30 days without a check, an ignored older manifest, a development copy…
    pub info: Vec<String>,
    /// English, under "technical details".
    pub details: String,
    /// Links in the body ("リリースノートを開く").
    pub links: Vec<ActionView>,
    pub buttons: Vec<ActionView>,
}

/// The page's view (design m5b E.2).
pub fn update_page(state: &AppState, clock: Clock<'_>, lang: Lang) -> UpdatePageView {
    let page_state = rules::page_state(state);
    let params = params(state, lang);
    let installed = rules::installed(state);
    let offered = rules::release_version(state);
    let copy = rules::availability(state) != Availability::Available;
    let mut view = UpdatePageView {
        title: text::page_title(lang),
        details: details(state),
        ..UpdatePageView::default()
    };
    let buttons = |actions: &[UpdateAction]| -> Vec<ActionView> {
        actions
            .iter()
            .map(|action_| action(*action_, &offered, lang))
            .collect()
    };
    let offer_lines = |view: &mut UpdatePageView| {
        if let Some(verified) = rules::cached_verified(state) {
            view.version_line =
                text::new_version(&verified.version.to_string(), &installed.to_string(), lang);
            view.published = text::published(&date(verified.issued_at, clock, lang), lang);
            view.links = buttons(&[UpdateAction::ReleaseNotes]);
        }
    };
    let failure = |view: &mut UpdatePageView, explained: Explained| {
        view.status = words(explained.msg, &params, lang);
        view.tone = tone_of(explained.msg);
        view.buttons = buttons(&explained.actions);
    };
    match page_state {
        PageState::Unavailable => {
            if let Some(explained) = explain_availability(&rules::availability(state)) {
                view.status = words(explained.msg, &params, lang);
                view.tone = Tone::Info;
                view.buttons = buttons(&explained.actions);
            }
        }
        PageState::Inconsistent => {
            view.status = words(Msg::Inconsistent, &params, lang);
            view.tone = Tone::Warning;
            let mut actions = Vec::new();
            if rules::ready_offer(state).is_some() {
                actions.push(UpdateAction::RunInstaller);
            }
            actions.push(UpdateAction::ReleasePage);
            view.buttons = buttons(&actions);
        }
        PageState::Session => {
            offer_lines(&mut view);
            let (step, progress, cancel) = match state.session {
                SessionPhase::Updating {
                    stage: UpdateStage::Sending { sent, total },
                    ..
                } => {
                    let percent = percent(sent, total);
                    (text::preparing(percent, lang), Some(percent), true)
                }
                SessionPhase::Updating {
                    stage: UpdateStage::StartingRunner,
                    ..
                } => (text::windows_checking(lang), Some(100), false),
                SessionPhase::Updating {
                    stage: UpdateStage::HandedOff,
                    ..
                } => (text::handing_over(lang), Some(100), false),
                _ => (text::waiting_for_permission(lang), None, true),
            };
            view.status = step;
            view.progress = progress;
            view.progress_label = view.status.clone();
            view.progress_announcement = match progress {
                Some(percent) if percent < 100 => text::preparing(percent / 25 * 25, lang),
                _ => view.status.clone(),
            };
            view.buttons = buttons(&[UpdateAction::Cancel]);
            view.buttons[0].enabled = cancel;
        }
        PageState::Checking => {
            view.status = text::checking(lang);
            view.buttons = buttons(&[UpdateAction::Cancel]);
        }
        PageState::Downloading => {
            offer_lines(&mut view);
            if let DownloadPhase::Downloading {
                received, total, ..
            } = state.update.download
            {
                let percent = percent(received, total);
                view.progress = Some(percent);
                view.progress_text = text::downloading(received, total, percent, lang);
                view.progress_label = text::download_progress_label(percent, lang);
                view.progress_announcement = text::download_progress_label(percent / 25 * 25, lang);
            }
            view.buttons = buttons(&[UpdateAction::Cancel]);
        }
        PageState::SessionFailed => {
            offer_lines(&mut view);
            if let Some(end) = &state.update.session_end {
                failure(&mut view, explain_session_end(end));
            }
        }
        PageState::DownloadFailed => {
            offer_lines(&mut view);
            if let Some(error) = &state.update.download_error {
                failure(&mut view, explain_download(error));
            }
        }
        PageState::Ready => {
            offer_lines(&mut view);
            let size = rules::offer(state).map_or(0, |offer| offer.verified.asset.size);
            view.progress_text = text::downloaded(size, lang);
            let prompt = !state.elevated;
            let helper = state
                .update
                .env
                .as_ref()
                .map(|env| {
                    env.install_dir
                        .join("mklm-helper.exe")
                        .display()
                        .to_string()
                })
                .unwrap_or_default();
            view.explanation = text::update_explanation(&helper, prompt, lang);
            view.buttons = buttons(&[
                UpdateAction::Skip,
                UpdateAction::Later,
                UpdateAction::UpdateNow,
            ]);
            if let Some(update) = view.buttons.last_mut() {
                if prompt {
                    update.text = text::button(Button::UpdateNowPrompt, lang);
                }
                update.primary = true;
                update.enabled = rules::update_block(state).is_none();
            }
            if let Some(UpdateBlock::Journal(refusal)) = rules::update_block(state) {
                let explained = explain_refusal(&refusal);
                view.info.push(words(explained.msg, &params, lang));
                view.buttons.extend(buttons(&explained.actions));
            }
        }
        PageState::Skipped | PageState::NotDownloaded => {
            offer_lines(&mut view);
            view.status = if page_state == PageState::Skipped {
                text::skipped(&offered.to_string(), lang)
            } else {
                text::not_downloaded(lang)
            };
            view.buttons = if copy {
                buttons(&[UpdateAction::CheckNow])
            } else {
                buttons(&[UpdateAction::Download, UpdateAction::CheckNow])
            };
        }
        PageState::Manual => {
            offer_lines(&mut view);
            view.links.clear();
            failure(
                &mut view,
                explain_refusal(&UpdateRefusal::ManualUpdateRequired {
                    min_from: String::new(),
                    installed: String::new(),
                }),
            );
            view.tone = Tone::Info;
        }
        PageState::UpToDate => {
            if let Some(verified) = rules::cached_verified(state) {
                view.status = text::up_to_date(
                    &installed.to_string(),
                    &date(verified.issued_at, clock, lang),
                    lang,
                );
            }
            view.tone = Tone::Success;
            view.buttons = buttons(&[UpdateAction::CheckNow]);
            view.buttons[0].primary = true;
            // A later check failed (an older manifest ignored…): its way out stays at hand (E.6).
            if let Some(error) = &state.update.check_error {
                let explained = explain_check(error, not_found_days(state));
                for extra in explained.actions {
                    if !matches!(extra, UpdateAction::CheckNow | UpdateAction::Download)
                        && !view.buttons.iter().any(|button| button.action == extra)
                    {
                        view.buttons.push(action(extra, &offered, lang));
                    }
                }
                if !view.details.is_empty()
                    && !view
                        .buttons
                        .iter()
                        .any(|button| button.action == UpdateAction::CopyDetails)
                {
                    view.buttons
                        .push(action(UpdateAction::CopyDetails, &offered, lang));
                }
            }
        }
        PageState::CheckFailed => {
            if let Some(error) = &state.update.check_error {
                let mut explained = explain_check(error, not_found_days(state));
                // A failure worth reporting keeps "詳細をコピー" (E.2 "失敗").
                if explained.actions.is_empty() {
                    explained.actions.push(UpdateAction::CheckNow);
                }
                failure(&mut view, explained);
            }
        }
        PageState::NotChecked => {
            view.status = text::not_checked(lang);
            view.buttons = buttons(&[UpdateAction::CheckNow]);
        }
    }
    view.info
        .extend(info_lines(state, page_state, &params, clock, lang));
    if !view.details.is_empty()
        && !view
            .buttons
            .iter()
            .any(|button| button.action == UpdateAction::CopyDetails)
        && matches!(
            page_state,
            PageState::CheckFailed | PageState::SessionFailed | PageState::DownloadFailed
        )
    {
        view.buttons
            .push(action(UpdateAction::CopyDetails, &offered, lang));
    }
    // The last enabled button is the primary one when none is.
    if !view.buttons.iter().any(|button| button.primary)
        && let Some(primary) = view.buttons.iter_mut().rev().find(|button| button.enabled)
    {
        primary.primary = true;
    }
    view
}

/// The lines under the page's body (E.2): whatever applies.
fn info_lines(
    state: &AppState,
    page_state: PageState,
    params: &Params,
    clock: Clock<'_>,
    lang: Lang,
) -> Vec<String> {
    let mut lines = Vec::new();
    if matches!(
        rules::availability(state),
        Availability::NotInstalledCopy { .. }
    ) {
        lines.push(words(Msg::NotInstalledCopy, params, lang));
    }
    if let Some(note) = &state.update.note {
        lines.push(match note {
            UpdateNote::Cancelled => words(Msg::Cancelled, params, lang),
            UpdateNote::InstallerFailed(_) => words(Msg::InstallerNotStarted, params, lang),
        });
    }
    if let Some(verified) = rules::cached_verified(state)
        && (verified.freshness == Freshness::Expired
            || state.now_unix.is_some_and(|now| now > verified.expires))
    {
        let params = Params {
            date: date(verified.expires, clock, lang),
            ..params.clone()
        };
        lines.push(words(Msg::Expired, &params, lang));
    }
    if stale(state) {
        lines.push(words(
            if stale_structural(state) {
                Msg::StaleStructural
            } else {
                Msg::Stale
            },
            params,
            lang,
        ));
    }
    let client = &state.update.client;
    if let Some(note) = &client.last_rollback
        && client.last_check == Some(note.at)
    {
        lines.push(text::rollback_ignored(
            &date(note.issued_at, clock, lang),
            &date(note.seen, clock, lang),
            lang,
        ));
    }
    // A failed check while an earlier result is shown.
    if let Some(error) = &state.update.check_error
        && page_state != PageState::CheckFailed
        && page_state != PageState::Unavailable
    {
        let explained = explain_check(error, not_found_days(state));
        lines.push(text::last_check_failed(
            &words(explained.msg, params, lang),
            lang,
        ));
    }
    if state.update.env.as_ref().is_some_and(|env| env.arm64_pc) {
        lines.push(text::arm64_available(lang));
    }
    lines
}

fn percent(done: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    u32::try_from((done.min(total) * 100) / total).unwrap_or(100)
}

fn not_found_days(state: &AppState) -> u64 {
    let now = state.now_unix.unwrap_or(0);
    state
        .update
        .client
        .last_failure
        .as_ref()
        .map_or(0, |failure| now.saturating_sub(failure.first_at) / 86_400)
}

/// No successful check for 30 days while checks keep failing (E.3, OPS-UX-TEST-5).
fn stale(state: &AppState) -> bool {
    let now = state.now_unix.unwrap_or(0);
    let client = &state.update.client;
    let since = client
        .last_success
        .or(client.last_failure.as_ref().map(|failure| failure.first_at));
    client.last_failure.is_some()
        && since.is_some_and(|since| {
            now.saturating_sub(since) >= mklm_update::run::timing::STALE_NOTICE_DAYS * 86_400
        })
}

fn stale_structural(state: &AppState) -> bool {
    state
        .update
        .client
        .last_failure
        .as_ref()
        .is_some_and(|failure| failure.class == ErrorClass::Structural)
}

/// The tone of a failure's message: structural ones warn, the rest inform.
fn tone_of(msg: Msg) -> Tone {
    match msg {
        Msg::Cancelled | Msg::NotNewer | Msg::VersionChanged => Tone::Info,
        Msg::Installed | Msg::InstalledOther | Msg::InterruptedInstalled => Tone::Success,
        Msg::Sig | Msg::Rollback | Msg::Inconsistent | Msg::Timeout | Msg::AvBlocked => {
            Tone::Danger
        }
        _ => Tone::Warning,
    }
}

/// English diagnostics of what the page or the result shows (the technical details, "詳細をコピー").
pub fn details(state: &AppState) -> String {
    let mut lines = Vec::new();
    let update = &state.update;
    if let Availability::Unknown { detail } = rules::availability(state) {
        lines.push(format!("updates: {detail}"));
    }
    if let Some(error) = &update.check_error {
        lines.push(format!("check: {error}"));
        if let Some((class, id)) = classify::classify(error, not_found_days(state)) {
            lines.push(format!("class: {class:?}, {id}"));
        }
    }
    if let Some(error) = &update.download_error {
        lines.push(format!("download: {error}"));
    }
    if let Some(end) = &update.session_end {
        lines.push(match end {
            UpdateSessionEnd::NotLaunched(error) => format!("update session: {error}"),
            UpdateSessionEnd::NotReady(error) => format!("update session: not ready: {error}"),
            UpdateSessionEnd::Stage(StageEnd::Refused(refusal)) => {
                format!("update session: refused: {refusal}")
            }
            UpdateSessionEnd::Stage(end) => format!("update session: {end:?}"),
        });
    }
    if let Some(UpdateNote::InstallerFailed(detail)) = &update.note {
        lines.push(format!("installer: {detail}"));
    }
    if let Some(ShownResult::Result { result, .. }) = &update.shown {
        lines.push(format!(
            "update {}: {} -> {}, {}",
            result.run_id.as_str(),
            result.from_version,
            result.to_version,
            mklm_client::update::status::outcome_diagnostic(&result.outcome)
        ));
        if let Some(code) = result.installer_exit {
            lines.push(text::installer_exit_code(code, Lang::En));
        }
        if let UpdateOutcome::Interrupted { phase } = &result.outcome {
            lines.push(format!("phase: {}", text::phase(*phase, Lang::En)));
        }
    }
    if rules::inconsistent(state) {
        lines.push("the installed programs are of different versions".to_string());
    }
    if !lines.is_empty() {
        lines.push(format!(
            "MKLM {} ({})",
            rules::installed(state),
            mklm_update::Arch::of_this_build().as_str()
        ));
    }
    lines.join("\n")
}

/// The main screen's banner (design m5b E.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BannerView {
    pub text: String,
    pub tone: Tone,
    pub actions: Vec<ActionView>,
}

pub fn banner(state: &AppState, lang: Lang) -> Option<BannerView> {
    let version = rules::release_version(state);
    let params = params(state, lang);
    let details = action(UpdateAction::Details, &version, lang);
    let release = action(UpdateAction::ReleasePage, &version, lang);
    Some(match rules::banner(state)? {
        Banner::Available => BannerView {
            text: text::banner_available(&version.to_string(), lang),
            tone: Tone::Info,
            actions: vec![details],
        },
        Banner::Notice(UpdateNotice::Stale { structural: false }) => BannerView {
            text: words(Msg::Stale, &params, lang),
            tone: Tone::Info,
            actions: vec![details],
        },
        Banner::Notice(UpdateNotice::Stale { structural: true } | UpdateNotice::Expired { .. }) => {
            BannerView {
                text: words(Msg::StaleStructural, &params, lang),
                tone: Tone::Info,
                actions: vec![release, details],
            }
        }
        Banner::Notice(UpdateNotice::Rollback { .. }) => BannerView {
            text: text::banner_rollback(lang),
            tone: Tone::Warning,
            actions: vec![details],
        },
    })
}

/// The settings page's "更新" section (design m5b E.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateSettingsView {
    pub auto_check: bool,
    pub last_check: String,
    pub last_success: String,
    /// "30 日以上、更新を確認できていません（…）。"; empty when not.
    pub stale: String,
    /// The expiry line; empty while fresh.
    pub expired: String,
    /// "今すぐ確認" can be pressed.
    pub can_check: bool,
}

pub fn settings_section(state: &AppState, clock: Clock<'_>, lang: Lang) -> UpdateSettingsView {
    let client = &state.update.client;
    let when = |at: Option<u64>| at.map(|at| date_time(at, clock, lang));
    let offered = rules::offer(state).map(|offer| offer.verified.version.to_string());
    let found = match (&state.update.check_error, &state.update.outcome) {
        (Some(_), _) => Some(text::SettingsFound::Failed),
        (None, Some(CheckOutcome::Available(_))) => {
            offered.as_deref().map(text::SettingsFound::Available)
        }
        (None, Some(_)) => Some(text::SettingsFound::UpToDate),
        (None, None) => None,
    };
    let result = client
        .last_check
        .and(found)
        .map(|found| text::settings_check_result(found, lang));
    let params = params(state, lang);
    let expired = rules::cached_verified(state)
        .filter(|verified| {
            verified.freshness == Freshness::Expired
                || state.now_unix.is_some_and(|now| now > verified.expires)
        })
        .map(|verified| {
            words(
                Msg::Expired,
                &Params {
                    date: date(verified.expires, clock, lang),
                    ..params.clone()
                },
                lang,
            )
        })
        .unwrap_or_default();
    UpdateSettingsView {
        auto_check: state.settings.update.auto_check,
        last_check: text::settings_last_check(
            when(client.last_check).as_deref(),
            result.as_deref(),
            lang,
        ),
        last_success: text::settings_last_success(when(client.last_success).as_deref(), lang),
        stale: if stale(state) {
            text::settings_stale(stale_structural(state), lang)
        } else {
            String::new()
        },
        expired,
        can_check: rules::can_check(state)
            && !matches!(state.update.check, CheckPhase::Checking { .. }),
    }
}

/// The hand-off overlay (design m5b E.4 step 3).
pub fn handoff(lang: Lang) -> (String, String) {
    (text::handoff_title(lang), text::handoff_text(lang))
}

/// The result overlay (design m5b D.13, E.5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResultOverlayView {
    pub title: String,
    pub message: String,
    pub tone: Tone,
    pub details: String,
    pub actions: Vec<ActionView>,
}

pub fn result_overlay(state: &AppState, lang: Lang) -> Option<ResultOverlayView> {
    let shown = state.update.shown.as_ref()?;
    let params = params(state, lang);
    let version = rules::release_version(state);
    let (message, tone, mut actions) = match shown {
        ShownResult::Inconsistent { cached, .. } => {
            let mut actions = Vec::new();
            if *cached {
                actions.push(UpdateAction::RunInstaller);
            }
            actions.extend([UpdateAction::ReleasePage, UpdateAction::CopyDetails]);
            (
                words(Msg::Inconsistent, &params, lang),
                Tone::Warning,
                actions,
            )
        }
        ShownResult::Result {
            result,
            own,
            closed_for_update,
        } => {
            let result_params = Params {
                installed: result
                    .installed_version
                    .clone()
                    .unwrap_or_else(|| result.from_version.clone()),
                offered: result.to_version.clone(),
                ..params.clone()
            };
            let explained = explain_outcome(result, *own);
            let mut message = match &result.outcome {
                UpdateOutcome::Failed(FailedReason::InstallerTimedOut) => {
                    words(Msg::Timeout, &result_params, lang)
                }
                UpdateOutcome::Failed(_) if !rules::inconsistent(state) => text::join_sentences(
                    &text::version_now(&rules::installed(state).to_string(), lang),
                    &text::keyboards_unchanged(lang),
                    lang,
                ),
                // The runner refused (D.7 steps 8–10): the refusal's text, which does not say
                // what stayed as it was.
                UpdateOutcome::NotInstalled(NotInstalledReason::Refused(_)) => {
                    text::with_version_kept(
                        words(explained.msg, &result_params, lang),
                        &result_params.installed,
                        lang,
                    )
                }
                _ => words(explained.msg, &result_params, lang),
            };
            // Whatever message they map to, "not installed" and "failed" say the keyboards did
            // not change (E.6).
            if matches!(
                result.outcome,
                UpdateOutcome::NotInstalled(_) | UpdateOutcome::Failed(_)
            ) {
                message = text::with_keyboards_unchanged(message, lang);
            }
            if *closed_for_update {
                message =
                    text::join_sentences(&message, &text::closed_for_another_update(lang), lang);
            }
            (message, tone_of(explained.msg), explained.actions)
        }
    };
    actions.retain(|action| *action != UpdateAction::Close);
    let mut actions: Vec<ActionView> = actions
        .into_iter()
        .map(|action_| action(action_, &version, lang))
        .collect();
    actions.push(ActionView {
        primary: true,
        ..action(UpdateAction::Close, &version, lang)
    });
    Some(ResultOverlayView {
        title: text::result_title(lang),
        message,
        tone,
        details: details(state),
        actions,
    })
}

/// The tray while an update is ready (design m5b E.3): the tooltip, and whether "MKLM を更新…" is
/// on.
pub fn tray(state: &AppState, lang: Lang) -> (Option<String>, bool) {
    let tooltip = rules::ready_offer(state)
        .filter(|offer| !offer.skipped)
        .map(|offer| text::tray_tooltip(&offer.verified.version.to_string(), lang));
    (tooltip, rules::tray_can_update(state))
}

/// The line of the UAC explanation about the prompt's program (design m5b E.4; SECURITY-14):
/// only when the explanation comes before an update.
pub fn uac_location_line(state: &AppState, lang: Lang) -> String {
    if state.uac_origin != crate::state::UacNoticeOrigin::Update {
        return String::new();
    }
    let helper = state
        .update
        .env
        .as_ref()
        .map(|env| {
            env.install_dir
                .join("mklm-helper.exe")
                .display()
                .to_string()
        })
        .unwrap_or_default();
    text::location_check(&helper, lang)
}

impl SnapshotText for ActionView {
    fn snapshot_text(&self) -> String {
        let mut line = format!("[{}]", self.text);
        if self.label != self.text {
            line.push_str(&format!(" (read as {})", self.label));
        }
        if self.primary {
            line.push_str(" primary");
        }
        if !self.enabled {
            line.push_str(" off");
        }
        line
    }
}

impl SnapshotText for UpdatePageView {
    fn snapshot_text(&self) -> String {
        let mut out = format!("title: {}\n", self.title);
        let mut field = |label: &str, value: &str| {
            if !value.is_empty() {
                out.push_str(&format!("{label}: {value}\n"));
            }
        };
        field("status", &self.status);
        field("version", &self.version_line);
        field("published", &self.published);
        field("progress text", &self.progress_text);
        field("explanation", &self.explanation);
        for line in &self.info {
            out.push_str(&format!("info: {line}\n"));
        }
        if let Some(progress) = self.progress {
            out.push_str(&format!("progress: {progress} ({})\n", self.progress_label));
        }
        out.push_str(&format!("tone: {:?}\n", self.tone));
        for link in &self.links {
            out.push_str(&format!("link: {}\n", link.snapshot_text()));
        }
        for button in &self.buttons {
            out.push_str(&format!("button: {}\n", button.snapshot_text()));
        }
        if !self.details.is_empty() {
            out.push_str(&format!("details: {}\n", self.details.replace('\n', " | ")));
        }
        out
    }
}

impl SnapshotText for BannerView {
    fn snapshot_text(&self) -> String {
        let mut out = format!("banner: {}\ntone: {:?}\n", self.text, self.tone);
        for action in &self.actions {
            out.push_str(&format!("button: {}\n", action.snapshot_text()));
        }
        out
    }
}

impl SnapshotText for UpdateSettingsView {
    fn snapshot_text(&self) -> String {
        let mut out = format!(
            "auto check: {}\nlast check: {}\nlast success: {}\n",
            if self.auto_check { "☑" } else { "☐" },
            self.last_check,
            self.last_success
        );
        if !self.stale.is_empty() {
            out.push_str(&format!("stale: {}\n", self.stale));
        }
        if !self.expired.is_empty() {
            out.push_str(&format!("expired: {}\n", self.expired));
        }
        out.push_str(&format!("can check: {}\n", self.can_check));
        out
    }
}

impl SnapshotText for ResultOverlayView {
    fn snapshot_text(&self) -> String {
        let mut out = format!(
            "title: {}\nmessage: {}\ntone: {:?}\n",
            self.title, self.message, self.tone
        );
        for action in &self.actions {
            out.push_str(&format!("button: {}\n", action.snapshot_text()));
        }
        if !self.details.is_empty() {
            out.push_str(&format!("details: {}\n", self.details.replace('\n', " | ")));
        }
        out
    }
}

#[cfg(test)]
mod tests;
