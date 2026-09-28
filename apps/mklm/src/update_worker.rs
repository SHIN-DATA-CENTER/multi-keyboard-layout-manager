//! The update worker (design m5b E.1 "スレッド"): one long-lived thread, `mklm-update`, for what
//! updates do besides the update session — the environment and the records at start, the checks,
//! the download, the cache's clean-up, the release page and the interactive installer. Checks and
//! downloads never run on the I/O worker: a slow network must not hold up reading the keyboards.
//!
//! Tasks run in order. A check or a download stops at the next read when the UI thread sets the
//! cancel flag ([`UpdateWorker::cancel`]). Answers go to the UI thread as `AppMsg::Update`.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::SystemTime;

use mklm_client::update::cache::UpdateCache;
use mklm_client::update::check::{CheckError, CheckOutcome, check, reverify_cached};
use mklm_client::update::download::{DownloadError, download};
use mklm_client::update::env::{UpdateEnv, environment};
use mklm_client::update::status::read_status;
use mklm_core::Timestamp;
use mklm_update::fetch::{FetchError, Transport};
use mklm_update::run::{RunView, interrupted_result};
use mklm_update::url::Endpoints;
use mklm_update::{Arch, TrustState};
use mklm_win::os::NativeMachine;

use crate::app::post;
use crate::log;
use crate::state::AppMsg;
use crate::state::update::{
    EnvView, InstallerRunError, MachineView, UpdateMsg, UpdateStart, UpdateTask,
};

/// The UI thread's handle on the worker.
#[derive(Debug)]
pub struct UpdateWorker {
    tasks: Sender<UpdateTask>,
    cancel: Arc<AtomicBool>,
}

impl UpdateWorker {
    /// Starts the worker for `endpoints` (GitHub; a development build's `--update-endpoint`).
    pub fn start(endpoints: Endpoints) -> io::Result<UpdateWorker> {
        let (tasks, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        thread::Builder::new()
            .name("mklm-update".into())
            .spawn(move || run(receiver, &worker_cancel, endpoints))?;
        Ok(UpdateWorker { tasks, cancel })
    }

    pub fn send(&self, task: UpdateTask) {
        let _ = self.tasks.send(task);
    }

    /// Stops the running check or download at its next read.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn run(tasks: Receiver<UpdateTask>, cancel: &AtomicBool, endpoints: Endpoints) {
    let loopback = endpoints != Endpoints::production();
    let env = environment(env!("CARGO_PKG_VERSION"), endpoints);
    log::info(format!(
        "updates: {:?}, installed {} ({})",
        env.availability,
        env.installed,
        env.arch.as_str()
    ));
    let cache = UpdateCache::new(env.cache_dir.clone());
    let mut transport: Option<Box<dyn Transport>> = None;
    while let Ok(task) = tasks.recv() {
        match task {
            UpdateTask::Start => {
                if let Err(error) = cache.remove_partials() {
                    log::warn(format!("removing unfinished downloads failed: {error}"));
                }
                post(AppMsg::Update(UpdateMsg::Started(Box::new(start(
                    &env, &cache,
                )))));
            }
            UpdateTask::Check {
                token,
                manual,
                skipped,
            } => {
                cancel.store(false, Ordering::SeqCst);
                let machine = machine_trust(&env, &cache);
                let result = match connection(&mut transport, loopback) {
                    Ok(transport) => check(
                        transport,
                        &env,
                        &cache,
                        &machine,
                        skipped.as_ref(),
                        now_unix(),
                        cancel,
                    ),
                    Err(error) => Err(CheckError::Fetch(error)),
                };
                log_check(manual, &result);
                post(AppMsg::Update(UpdateMsg::Checked {
                    token,
                    result: Box::new(result),
                    client: Box::new(cache.load_state()),
                }));
            }
            UpdateTask::Download { token, offer } => {
                cancel.store(false, Ordering::SeqCst);
                let mut last_percent = u64::MAX;
                let result = match connection(&mut transport, loopback) {
                    Ok(transport) => download(
                        transport,
                        &env,
                        &cache,
                        &offer,
                        &mut |received, total| {
                            let percent = received * 100 / total.max(1);
                            if percent != last_percent {
                                last_percent = percent;
                                post(AppMsg::Update(UpdateMsg::DownloadProgress {
                                    token,
                                    received,
                                    total,
                                }));
                            }
                        },
                        cancel,
                    ),
                    Err(error) => Err(DownloadError::Fetch(error)),
                };
                match &result {
                    Ok(path) => log::info(format!("update downloaded: {}", path.display())),
                    Err(error) => log::warn(format!("update download: {error}")),
                }
                post(AppMsg::Update(UpdateMsg::Downloaded {
                    token,
                    result: Box::new(result),
                }));
            }
            UpdateTask::ReadStatus => {
                post(AppMsg::Update(UpdateMsg::StatusRead(Box::new(
                    machine_view(&env, &cache),
                ))));
            }
            UpdateTask::Prune { keep } => {
                if let Err(error) = cache.prune(keep.as_deref()) {
                    log::warn(format!("cleaning the update cache failed: {error}"));
                }
            }
            UpdateTask::OpenReleasePage(version) => {
                if let Err(error) = mklm_win::ui::open_url::open_release_page(&version) {
                    log::warn(format!(
                        "opening the release page of {version} failed: {error}"
                    ));
                }
            }
            UpdateTask::RunInstaller => {
                let result = run_installer(&env, &cache);
                if let Err(error) = &result {
                    log::warn(format!("running the installer: {error:?}"));
                }
                post(AppMsg::Update(UpdateMsg::InstallerRan(result)));
            }
        }
    }
}

/// The transport, made once (WinHTTP with the automatic proxy; without a proxy for a loopback
/// rehearsal server, design m5b A.10).
fn connection(
    transport: &mut Option<Box<dyn Transport>>,
    loopback: bool,
) -> Result<&mut dyn Transport, FetchError> {
    if transport.is_none() {
        let agent = mklm_update::user_agent(env!("CARGO_PKG_VERSION"), Arch::of_this_build());
        let made = make_transport(&agent, loopback).map_err(FetchError::Transport)?;
        *transport = Some(made);
    }
    Ok(transport
        .as_mut()
        .map(|transport| transport.as_mut() as &mut dyn Transport)
        .expect("made above"))
}

fn make_transport(
    agent: &str,
    loopback: bool,
) -> Result<Box<dyn Transport>, mklm_update::fetch::TransportError> {
    use mklm_update::winhttp::WinHttpTransport;
    #[cfg(all(debug_assertions, mklm_update_dev))]
    if loopback {
        return WinHttpTransport::new_without_proxy(agent)
            .map(|transport| Box::new(transport) as Box<dyn Transport>);
    }
    let _ = loopback;
    WinHttpTransport::new(agent).map(|transport| Box::new(transport) as Box<dyn Transport>)
}

/// The machine's trust record (a record that cannot be read is empty, as the helper reads it).
fn machine_trust(env: &UpdateEnv, cache: &UpdateCache) -> TrustState {
    read_status(&env.install_dir, cache).machine_trust
}

/// The machine's records as the GUI shows them.
fn machine_view(env: &UpdateEnv, cache: &UpdateCache) -> MachineView {
    let status = read_status(&env.install_dir, cache);
    for warning in &status.warnings {
        log::warn(format!("update records: {warning}"));
    }
    let interrupted = match &status.run {
        RunView::Interrupted(record) => Some(interrupted_result(
            record,
            Timestamp(now_unix() * 1000),
            &status.install,
        )),
        RunView::Idle | RunView::InProgress { .. } => None,
    };
    MachineView {
        run: status.run.clone(),
        last_result: status.last_result.clone(),
        interrupted,
        consistent: status.install.consistent_version(),
        install_known: status.install.gui.is_some()
            || status.install.cli.is_some()
            || status.install.helper.is_some(),
    }
}

/// What the start reads (design m5b D.13, E.1).
fn start(env: &UpdateEnv, cache: &UpdateCache) -> UpdateStart {
    let machine = machine_view(env, cache);
    let cached = cache
        .load_manifest()
        .map(|_| reverify_cached(env, cache, &machine_trust(env, cache), None, now_unix()));
    if let Some(Err(error)) = &cached {
        log::info(format!(
            "the cached update information does not verify now: {error}"
        ));
    }
    UpdateStart {
        env: EnvView {
            installed: env.installed.clone(),
            availability: env.availability.clone(),
            install_dir: env.install_dir.clone(),
            arm64_pc: env.native == Some(NativeMachine::Arm64) && env.arch == Arch::X64,
        },
        machine,
        client: cache.load_state(),
        cached,
    }
}

/// "インストーラーを実行" (design m5b D.13 step 4): the cached manifest verified again, the
/// installer's size and SHA-256 compared — which only catches a damaged file (RED-TEAM-2) — then
/// the interactive installer with UAC.
fn run_installer(env: &UpdateEnv, cache: &UpdateCache) -> Result<(), InstallerRunError> {
    let failed = |detail: String| InstallerRunError::Failed(detail);
    let outcome = reverify_cached(env, cache, &machine_trust(env, cache), None, now_unix())
        .map_err(|error| failed(error.to_string()))?;
    let verified = outcome.verified();
    let path: PathBuf = cache.matching_installer(&verified.asset).ok_or_else(|| {
        failed(format!(
            "{} is not in the update cache",
            verified.asset.name
        ))
    })?;
    match mklm_win::ui::open_url::run_installer_interactive(&path) {
        Ok(()) => Ok(()),
        Err(mklm_win::Error::Cancelled) => Err(InstallerRunError::Cancelled),
        Err(error) => Err(failed(error.to_string())),
    }
}

/// The log keeps what a check found (design m3 F.8), both values of an ignored older manifest
/// (design m5b E.3).
fn log_check(manual: bool, result: &Result<CheckOutcome, CheckError>) {
    let how = if manual { "manual" } else { "automatic" };
    match result {
        Ok(outcome) => {
            let verified = outcome.verified();
            log::info(format!(
                "update check ({how}): {} {} issued {} expires {} ({:?})",
                match outcome {
                    CheckOutcome::UpToDate(_) => "up to date with",
                    CheckOutcome::Available(_) => "offers",
                    CheckOutcome::ManualRequired(_) => "needs a manual update to",
                },
                verified.version,
                verified.issued_at,
                verified.expires,
                verified.freshness
            ));
        }
        Err(CheckError::Refused(mklm_update::UpdateRefusal::Rollback { issued_at, seen })) => {
            log::warn(format!(
                "update check ({how}): an older manifest was ignored (issued_at {issued_at}, recorded {seen})"
            ));
        }
        Err(CheckError::Refused(refusal))
            if mklm_client::update::classify::is_unexpected_in_check(refusal) =>
        {
            log::error(format!(
                "update check ({how}): a refusal a check never returns (a bug): {refusal}"
            ));
        }
        Err(error) => log::warn(format!("update check ({how}): {error}")),
    }
}
