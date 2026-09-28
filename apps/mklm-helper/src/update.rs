//! H1: a `StageUpdate` in a pipe session (design m5b D.4) and the `RecordTrust` handling (C.4):
//! `mklm_ipc::staging::{stage_update, record_trust}` over a `StagerEnv` implemented with mklm-win.
//! Also what H1 and H2 share: the machine records, the write lock, the journal, the update log;
//! and the clean-up a normal session does once it holds the lock (D.11, D.13).
//!
//! The decisions are in the pure drivers (tested in mklm-ipc and mklm-update); this file only maps
//! each environment call onto mklm-win. The helper never opens a file in the user's profile: the
//! installer arrives over the pipe.

#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mklm_core::{BootId, Journal, Liveness, ProcessIdentity, Timestamp};
use mklm_engine::win::WinHost;
use mklm_engine::{Host, HostError};
use mklm_ipc::staging::{
    StageFlowEnd, StageLink, StageSink, StagedInstaller, StagerEnv, record_trust as record,
    stage_update,
};
use mklm_ipc::{HelperMessage, RunUpdateArgs, StageUpdateRequest, TrustReport, UpdateMessage};
use mklm_update::run::{
    InstallState, RunId, RunRecord, RunView, UpdateResult, classify_run, interrupted_result,
};
use mklm_update::version::parse_installed_version;
use mklm_update::{
    Arch, KeyError, RUNNER_EXE_NAME, Sha256Digest, Sha256Stream, TrustAnchors, TrustState,
    UpdateRefusal, Version,
};
use mklm_win::elevation::{SpawnedProcess, runner_environment, spawn_clean};
use mklm_win::journal_store::read_journal_store;
use mklm_win::os::fixed_install_dir;
use mklm_win::proc_identity::{
    current_process_identity, file_nt_path, process_identity, process_image_nt_path,
    process_liveness,
};
use mklm_win::protected_dir::{
    DataDir, FileLock, ProtectedDir, ensure_protected_dir, program_data_dir,
};
use mklm_win::update_dir::{
    RunDir, StagedFile, free_space, read_build_ids, remove_run_dir, sweep_stale_run_dirs,
};
use mklm_win::update_store::{
    LAST_RESULT_VALUE, RUN_VALUE, TRUST_VALUE, UpdateStore, read_update_store,
};
use mklm_win::{Error as WinError, session};

/// The helper's file name in the installation folder.
const HELPER_EXE: &str = "mklm-helper.exe";
/// `logs\update.log` is rotated to `update.log.1` beyond this size (design m5b D.7).
const LOG_ROTATE_AT: u64 = 1024 * 1024;

// ---- The update log ----

/// `%ProgramData%\SHIN DATA CENTER\MKLM\logs\update.log` (SYSTEM and Administrators only): the
/// steps, results and durations of H1 and H2, in English. Best effort: a log that cannot be
/// written never stops an update.
#[derive(Debug)]
pub(crate) struct UpdateLog {
    role: &'static str,
    started: Instant,
}

impl UpdateLog {
    pub(crate) fn new(role: &'static str) -> UpdateLog {
        UpdateLog {
            role,
            started: Instant::now(),
        }
    }

    pub(crate) fn line(&self, text: &str) {
        let _ = self.append(text);
    }

    fn append(&self, text: &str) -> Result<(), WinError> {
        let logs = ensure_protected_dir(DataDir::Logs)?;
        let path = logs.path().join("update.log");
        if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > LOG_ROTATE_AT) {
            let _ = std::fs::rename(&path, logs.path().join("update.log.1"));
        }
        let when = mklm_win::time::local_time(now())
            .map(|time| time.to_iso_text())
            .unwrap_or_default();
        let line = format!(
            "{when} {} pid {} +{} ms: {}\r\n",
            self.role,
            std::process::id(),
            self.started.elapsed().as_millis(),
            text.replace(['\r', '\n'], " ")
        );
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|error| io_error("CreateFileW", &error))?;
        file.write_all(line.as_bytes())
            .map_err(|error| io_error("WriteFile", &error))
    }
}

fn io_error(function: &'static str, error: &std::io::Error) -> WinError {
    WinError::Win32 {
        function,
        code: error.raw_os_error().map_or(0, |code| code as u32),
    }
}

/// Wall clock in milliseconds (display and records only).
pub(crate) fn now() -> Timestamp {
    Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            }),
    )
}

pub(crate) fn now_unix() -> u64 {
    now().0 / 1000
}

// ---- What H1 and H2 share ----

/// This build's version, architecture and keys, and this process (design m5b D.4 step 3, D.7
/// step 8). A build without keys refuses with `NotConfigured`.
pub(crate) struct Build {
    pub(crate) version: Version,
    pub(crate) arch: Arch,
    pub(crate) anchors: TrustAnchors,
    pub(crate) me: ProcessIdentity,
    pub(crate) boot: BootId,
    pub(crate) install_dir: PathBuf,
}

impl Build {
    pub(crate) fn current() -> Result<Build, UpdateRefusal> {
        let internal = |detail: String| UpdateRefusal::Internal { detail };
        let version = parse_installed_version(env!("CARGO_PKG_VERSION"))?;
        let anchors = TrustAnchors::for_this_build().map_err(|error| match error {
            KeyError::NotConfigured => UpdateRefusal::NotConfigured,
            other => internal(format!("the trust anchors of this build: {other}")),
        })?;
        let me = current_process_identity().map_err(|error| internal(error.to_string()))?;
        let boot = session::boot_id().map_err(|error| internal(error.to_string()))?;
        let install_dir = fixed_install_dir().map_err(|error| internal(error.to_string()))?;
        Ok(Build {
            version,
            arch: Arch::of_this_build(),
            anchors,
            me,
            boot,
            install_dir,
        })
    }

    pub(crate) fn install_state(&self) -> InstallState {
        InstallState::from_build_ids(read_build_ids(&self.install_dir))
    }
}

/// NT paths compare ignoring case, as the file system does.
pub(crate) fn same_path(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The machine records (`HKLM\…\MKLM\Update`), the write lock and the journal, as both helper
/// processes use them (design m5b D.6, D.4 steps 5–6, D.7 steps 9–10).
#[derive(Default)]
pub(crate) struct Machine {
    store: Option<UpdateStore>,
    base: Option<ProtectedDir>,
    lock: Option<FileLock>,
}

impl Machine {
    fn store(&mut self) -> Result<&UpdateStore, String> {
        if self.store.is_none() {
            self.store = Some(UpdateStore::open_or_create().map_err(|error| error.to_string())?);
        }
        self.store
            .as_ref()
            .ok_or_else(|| "the update key is not open".to_string())
    }

    pub(crate) fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal> {
        let storage = |error: WinError| UpdateRefusal::Storage {
            detail: error.to_string(),
        };
        let base = ensure_protected_dir(DataDir::Base).map_err(storage)?;
        let lock = FileLock::acquire(&base, timeout).map_err(|error| match error {
            WinError::Timeout { .. } => UpdateRefusal::Busy,
            other => storage(other),
        })?;
        self.base = Some(base);
        self.lock = Some(lock);
        Ok(())
    }

    pub(crate) fn release_lock(&mut self) {
        self.lock = None;
    }

    /// The journal as the engine reads it; a store of another layout is unreadable.
    pub(crate) fn read_journal(&mut self) -> Result<Journal, String> {
        let raw = read_journal_store().map_err(|error| error.to_string())?;
        if let Some(version) = raw.store_version
            && version != mklm_core::STORE_VERSION
        {
            return Err(format!("journal store version {version}"));
        }
        Ok(Journal::parse(&raw.ops, &raw.baselines))
    }

    /// A corrupted value reads as empty (logged): only administrators can write it (C.4).
    pub(crate) fn read_trust(&mut self, log: &UpdateLog) -> TrustState {
        match read_update_store() {
            Ok(raw) => match raw.trust {
                Some(json) => TrustState::parse(&json).unwrap_or_else(|error| {
                    log.line(&format!("Trust is unreadable, taken as empty: {error}"));
                    TrustState::default()
                }),
                None => TrustState::default(),
            },
            Err(error) => {
                log.line(&format!("Trust cannot be read, taken as empty: {error}"));
                TrustState::default()
            }
        }
    }

    pub(crate) fn write_trust(&mut self, trust: &TrustState) -> Result<(), String> {
        self.store()?
            .write(TRUST_VALUE, &trust.to_json())
            .map_err(|error| error.to_string())
    }

    /// A `Run` value that does not parse is logged and taken as absent (it is then replaced).
    pub(crate) fn read_run(&mut self, log: &UpdateLog) -> Result<Option<RunRecord>, String> {
        let raw = read_update_store().map_err(|error| error.to_string())?;
        Ok(raw.run.and_then(|json| match RunRecord::from_json(&json) {
            Ok(record) => Some(record),
            Err(error) => {
                log.line(&format!("Run is unreadable, taken as absent: {error}"));
                None
            }
        }))
    }

    pub(crate) fn write_run(&mut self, run: &RunRecord) -> Result<(), String> {
        self.store()?
            .write(RUN_VALUE, &run.to_json())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn delete_run(&mut self) -> Result<(), String> {
        self.store()?
            .delete(RUN_VALUE)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String> {
        self.store()?
            .write(LAST_RESULT_VALUE, &result.to_json())
            .map_err(|error| error.to_string())
    }
}

/// SHA-256 and length of everything `reader` yields.
pub(crate) fn hash_reader(reader: &mut dyn Read) -> Result<(u64, Sha256Digest), String> {
    let mut stream = Sha256Stream::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("ReadFile: {error}"))?;
        if read == 0 {
            return Ok((len, stream.finish()));
        }
        stream.update(&buffer[..read]);
        len += read as u64;
    }
}

// ---- H1 ----

/// Who asked: the pipe server's process (with its creation time) and its session.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CallerInfo {
    pub(crate) identity: Option<ProcessIdentity>,
    pub(crate) session: Option<u32>,
}

impl CallerInfo {
    pub(crate) fn of(pid: u32, session: Option<u32>) -> CallerInfo {
        CallerInfo {
            identity: process_identity(pid).ok().flatten(),
            session,
        }
    }
}

/// The answer to one `StageUpdate` (design m5b D.4). The session goes on after a refusal; after
/// `HandedOff` the helper exits 0.
pub(crate) fn stage(
    request: &StageUpdateRequest,
    link: &mut dyn StageLink,
    caller: CallerInfo,
) -> StageFlowEnd {
    let log = UpdateLog::new("H1");
    match Build::current() {
        Ok(build) => {
            let mut env = StagerMachine::new(build, caller, &log);
            stage_update(&mut env, link, request)
        }
        Err(refusal) => {
            log.line(&format!("stage: refused before anything: {refusal}"));
            match link.send(HelperMessage::Update(UpdateMessage::Refused(
                refusal.clone(),
            ))) {
                Ok(()) => StageFlowEnd::Refused(refusal),
                Err(_) => StageFlowEnd::CallerLeft,
            }
        }
    }
}

/// The answer to a `RecordTrust` (design m5b C.4); the session goes on whatever it is.
pub(crate) fn record_trust(report: &TrustReport) -> UpdateMessage {
    let log = UpdateLog::new("H1");
    match Build::current() {
        Ok(build) => {
            let mut env = StagerMachine::new(build, CallerInfo::default(), &log);
            record(&mut env, report)
        }
        Err(refusal) => UpdateMessage::TrustNotRecorded(refusal),
    }
}

/// H1's machine (design m5b D.4).
struct StagerMachine<'a> {
    build: Build,
    caller: CallerInfo,
    machine: Machine,
    updates: Option<ProtectedDir>,
    run_dir: Option<RunDir>,
    tmp: Option<PathBuf>,
    runner: Option<SpawnedProcess>,
    log: &'a UpdateLog,
}

impl<'a> StagerMachine<'a> {
    fn new(build: Build, caller: CallerInfo, log: &'a UpdateLog) -> StagerMachine<'a> {
        StagerMachine {
            build,
            caller,
            machine: Machine::default(),
            updates: None,
            run_dir: None,
            tmp: None,
            runner: None,
            log,
        }
    }

    fn updates(&mut self) -> Result<&ProtectedDir, String> {
        if self.updates.is_none() {
            self.updates =
                Some(ensure_protected_dir(DataDir::Updates).map_err(|error| error.to_string())?);
        }
        self.updates
            .as_ref()
            .ok_or_else(|| "Updates is not open".to_string())
    }

    fn run_dir(&self) -> Result<&RunDir, String> {
        self.run_dir
            .as_ref()
            .ok_or_else(|| "the run folder is not open".to_string())
    }
}

/// The installer file as `receive_installer` writes it.
struct StagedInstallerFile(StagedFile);

impl StageSink for StagedInstallerFile {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0
            .write_all(bytes)
            .map_err(|error| format!("WriteFile: {error}"))
    }
}

impl StagedInstaller for StagedInstallerFile {
    fn commit(self: Box<Self>) -> Result<(), String> {
        self.0.commit().map_err(|error| error.to_string())
    }
}

impl StagerEnv for StagerMachine<'_> {
    fn version(&self) -> &Version {
        &self.build.version
    }
    fn arch(&self) -> Arch {
        self.build.arch
    }
    fn anchors(&self) -> &TrustAnchors {
        &self.build.anchors
    }
    fn me(&self) -> ProcessIdentity {
        self.build.me
    }
    fn boot_id(&self) -> BootId {
        self.build.boot
    }
    fn now(&self) -> Timestamp {
        now()
    }
    fn now_unix(&self) -> u64 {
        now_unix()
    }
    fn runs_from_install_dir(&mut self) -> bool {
        let own = process_image_nt_path(self.build.me.pid);
        let installed = file_nt_path(&self.build.install_dir.join(HELPER_EXE));
        match (own, installed) {
            (Ok(own), Ok(installed)) => same_path(&own, &installed),
            _ => false,
        }
    }
    fn caller(&mut self) -> (Option<ProcessIdentity>, Option<u32>) {
        (self.caller.identity, self.caller.session)
    }
    fn free_space_program_data(&mut self) -> Result<u64, String> {
        let program_data = program_data_dir().map_err(|error| error.to_string())?;
        free_space(&program_data).map_err(|error| error.to_string())
    }
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal> {
        self.machine.acquire_lock(timeout)
    }
    fn release_lock(&mut self) {
        self.machine.release_lock();
    }
    fn read_journal(&mut self) -> Result<Journal, String> {
        self.machine.read_journal()
    }
    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        process_liveness(process)
    }
    fn read_install_state(&mut self) -> InstallState {
        self.build.install_state()
    }
    fn read_trust(&mut self) -> TrustState {
        self.machine.read_trust(self.log)
    }
    fn write_trust(&mut self, trust: &TrustState) -> Result<(), String> {
        self.machine.write_trust(trust)
    }
    fn read_run(&mut self) -> Result<Option<RunRecord>, String> {
        self.machine.read_run(self.log)
    }
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String> {
        self.machine.write_run(run)
    }
    fn delete_run(&mut self) -> Result<(), String> {
        self.machine.delete_run()
    }
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String> {
        self.machine.write_last_result(result)
    }
    fn sweep_run_dirs(&mut self, keep: Option<&RunId>) {
        let log = self.log;
        let swept = self.updates().and_then(|updates| {
            sweep_stale_run_dirs(updates, keep.map(RunId::as_str))
                .map_err(|error| error.to_string())
        });
        match swept {
            Ok(removed) if !removed.is_empty() => {
                log.line(&format!("removed old run folders {removed:?}"));
            }
            Ok(_) => {}
            Err(detail) => log.line(&format!("old run folders not swept: {detail}")),
        }
    }
    fn random_suffix(&mut self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        if session::random_bytes(&mut bytes).is_err() {
            // Uniqueness is all a run ID needs (its folder is created new, or the stage fails).
            let mixed =
                now().0 ^ (u64::from(self.build.me.pid) << 40) ^ self.build.me.creation_time;
            bytes = mixed.to_le_bytes();
            self.log
                .line("BCryptGenRandom failed; the run ID comes from the clock");
        }
        bytes
    }
    fn create_run_dir(&mut self, run_id: &RunId) -> Result<(), String> {
        let run_dir =
            RunDir::create(self.updates()?, run_id.as_str()).map_err(|error| error.to_string())?;
        self.run_dir = Some(run_dir);
        Ok(())
    }
    fn remove_run_dir(&mut self, run_id: &RunId) {
        // Nothing of this process may keep it open: the pins go first.
        self.run_dir = None;
        self.tmp = None;
        let log = self.log;
        let removed = self.updates().and_then(|updates| {
            remove_run_dir(updates, run_id.as_str()).map_err(|error| error.to_string())
        });
        if let Err(detail) = removed {
            log.line(&format!(
                "run folder {} not removed (the next sweep does): {detail}",
                run_id.as_str()
            ));
        }
    }
    fn write_run_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.run_dir()?
            .write_new(name, bytes)
            .map_err(|error| error.to_string())
    }
    fn create_installer(&mut self, name: &str) -> Result<Box<dyn StagedInstaller>, String> {
        let file = self
            .run_dir()?
            .create_exclusive(name)
            .map_err(|error| error.to_string())?;
        Ok(Box::new(StagedInstallerFile(file)))
    }
    fn copy_self_as_runner(&mut self) -> Result<(), String> {
        let source = self.build.install_dir.join(HELPER_EXE);
        let run_dir = self.run_dir()?;
        run_dir
            .copy_in(&source, RUNNER_EXE_NAME)
            .map_err(|error| error.to_string())?;
        let original = hash_file(&source)?;
        let mut copy = run_dir
            .open_locked(RUNNER_EXE_NAME, u64::MAX)
            .map_err(|error| error.to_string())?;
        let copied = hash_reader(&mut copy)?;
        drop(copy);
        if copied != original {
            let _ = run_dir.remove_file(RUNNER_EXE_NAME);
            return Err("the runner copy differs from mklm-helper.exe".to_string());
        }
        Ok(())
    }
    fn create_tmp_dir(&mut self) -> Result<(), String> {
        let tmp = self
            .run_dir()?
            .create_private_subdir("tmp")
            .map_err(|error| error.to_string())?;
        self.tmp = Some(tmp);
        Ok(())
    }
    fn spawn_runner(&mut self, run_id: &RunId) -> Result<ProcessIdentity, String> {
        let run_dir = self.run_dir()?;
        let exe = run_dir.path().join(RUNNER_EXE_NAME);
        let tmp = self
            .tmp
            .clone()
            .ok_or_else(|| "tmp was not made".to_string())?;
        let parameters = RunUpdateArgs {
            run_id: run_id.clone(),
        }
        .to_parameters();
        let environment = runner_environment(&tmp).map_err(|error| error.to_string())?;
        let runner = spawn_clean(&exe, &parameters, &environment, false)
            .map_err(|error| error.to_string())?;
        let identity = runner.identity().map_err(|error| error.to_string())?;
        self.runner = Some(runner);
        Ok(identity)
    }
    fn runner_exit_code(&mut self) -> Option<u32> {
        self.runner
            .as_ref()
            .and_then(|runner| runner.process.exit_code().ok().flatten())
    }
    fn stop_runner(&mut self, wait: Duration) {
        if let Some(runner) = &self.runner {
            let _ = runner.terminate(1);
            let _ = runner.process.wait(wait);
        }
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn log(&mut self, line: &str) {
        self.log.line(line);
    }
}

fn hash_file(path: &Path) -> Result<(u64, Sha256Digest), String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("CreateFileW: {error}"))?;
    hash_reader(&mut file)
}

// ---- A normal session's clean-up (design m5b D.11, D.13; RELIABILITY-11) ----

/// The engine's host, which also tidies the update records once it holds the write lock: a `Run`
/// whose owners are gone becomes `LastResult`, and run folders nothing uses are removed. Cheap and
/// best effort: a failure is logged and the request goes on.
pub(crate) struct TidyingHost {
    inner: WinHost,
    tidied: bool,
}

impl TidyingHost {
    pub(crate) fn new(inner: WinHost) -> TidyingHost {
        TidyingHost {
            inner,
            tidied: false,
        }
    }
}

/// Under the lock: the interrupted `Run` moved to `LastResult`, then the stale run folders swept.
fn tidy_under_lock() {
    let log = UpdateLog::new("session");
    // Keys are not needed here (nothing is verified), so a build without keys tidies too.
    let (Ok(boot), Ok(install_dir)) = (session::boot_id(), fixed_install_dir()) else {
        return;
    };
    let mut machine = Machine::default();
    let run = match machine.read_run(&log) {
        Ok(run) => run,
        Err(detail) => {
            log.line(&format!("tidy: Run cannot be read: {detail}"));
            return;
        }
    };
    let mut keep = run.as_ref().map(|run| run.run_id.clone());
    if let RunView::Interrupted(record) =
        classify_run(run.as_ref(), boot, &|process| process_liveness(process))
    {
        let after = InstallState::from_build_ids(read_build_ids(&install_dir));
        let result = interrupted_result(&record, now(), &after);
        match machine
            .write_last_result(&result)
            .and_then(|()| machine.delete_run())
        {
            Ok(()) => {
                log.line(&format!(
                    "tidy: interrupted run {} recorded",
                    record.run_id.as_str()
                ));
                keep = None;
            }
            Err(detail) => log.line(&format!("tidy: {detail}")),
        }
    }
    match ensure_protected_dir(DataDir::Updates)
        .and_then(|updates| sweep_stale_run_dirs(&updates, keep.as_ref().map(RunId::as_str)))
    {
        Ok(removed) if !removed.is_empty() => {
            log.line(&format!("tidy: removed old run folders {removed:?}"));
        }
        Ok(_) => {}
        Err(error) => log.line(&format!("tidy: run folders not swept: {error}")),
    }
}

impl Host for TidyingHost {
    type Lock = <WinHost as Host>::Lock;

    fn acquire_lock(&mut self, timeout: Duration) -> Result<Self::Lock, HostError> {
        let lock = self.inner.acquire_lock(timeout)?;
        if !self.tidied {
            self.tidied = true;
            tidy_under_lock();
        }
        Ok(lock)
    }

    fn now(&self) -> Timestamp {
        self.inner.now()
    }

    fn monotonic(&self) -> Duration {
        self.inner.monotonic()
    }

    fn new_op_id(&mut self) -> Result<mklm_core::OpId, HostError> {
        self.inner.new_op_id()
    }

    fn boot_id(&self) -> Result<BootId, HostError> {
        self.inner.boot_id()
    }

    fn boot_time_hint(&self) -> Option<u64> {
        self.inner.boot_time_hint()
    }

    fn current_process(&self) -> Result<ProcessIdentity, HostError> {
        self.inner.current_process()
    }

    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        self.inner.liveness(process)
    }

    fn system32_file_exists(&self, file_name: &str) -> Result<bool, HostError> {
        self.inner.system32_file_exists(file_name)
    }

    fn write_recovery_assets(
        &mut self,
        assets: &mklm_core::RecoveryAssets,
    ) -> Result<(), HostError> {
        self.inner.write_recovery_assets(assets)
    }

    fn drain_warnings(&mut self) -> Vec<String> {
        self.inner.drain_warnings()
    }

    fn take_recovery_assets_moved(&mut self) -> bool {
        self.inner.take_recovery_assets_moved()
    }
}
