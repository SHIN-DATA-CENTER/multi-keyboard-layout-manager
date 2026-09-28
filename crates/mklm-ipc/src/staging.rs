//! The helper's side of an update session, as pure drivers over environment traits (design m5b
//! D.3, D.4, C.4; OPS-UX-TEST-10): H1's [`stage_update`] and [`receive_installer`], the
//! `RecordTrust` handling [`record_trust`], and the order rule of the caller's messages
//! [`CallerOrder`]. The helper implements the traits over mklm-win
//! (`apps/mklm-helper/src/update.rs`); tests use fakes.
//!
//! H1 never trusts what the caller sends: it verifies the manifest itself against the machine
//! record and the keys of its own build, receives the installer bytes over the pipe (it never
//! opens a file in the user's profile), checks their size and SHA-256 against the verified
//! manifest, and only then copies itself as the update runner (H2) and hands the run over.

use std::time::{Duration, Instant};

use mklm_core::{BootId, ErrorCode, ErrorInfo, Journal, Liveness, ProcessIdentity, Timestamp};
use mklm_update::gate::check_journal;
use mklm_update::run::{
    InstallState, RECORD_SCHEMA, RunId, RunPhase, RunRecord, RunView, UpdateResult, classify_run,
    interrupted_result, space, timing,
};
use mklm_update::stage::{StagePlan, Stager};
use mklm_update::{
    Arch, MANIFEST_NAME, Purpose, SIGNATURE_NAME, Sha256Digest, TrustAnchors, TrustState,
    VerifiedManifest, VerifyInput, Version, apply_trust_report, verify_manifest,
};

use crate::frame::FrameError;
use crate::message::{CallerMessage, HelperMessage};
use crate::update::{CHUNK_LEN, StageUpdateRequest, TrustReport, UpdateMessage, UpdateRefusal};

/// H1 reports its progress every this many installer bytes (design m5b D.3).
pub const PROGRESS_EVERY: u64 = 1024 * 1024;
/// How often H1 reads `Run` while it waits for H2 to be ready (design m5b D.4 step 19).
pub const READY_POLL: Duration = Duration::from_millis(250);

pub trait StageLink {
    fn send(&mut self, message: HelperMessage) -> Result<(), String>;
    fn recv(&mut self, timeout: Duration) -> Result<CallerMessage, FrameError>;
}

pub trait StageSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// The installer file being received: `commit` flushes and closes it; dropped uncommitted, it is
/// deleted.
pub trait StagedInstaller: StageSink {
    fn commit(self: Box<Self>) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagingEnd {
    /// Every byte arrived and matched; the digest of what was written.
    Complete(Sha256Digest),
    /// Already sent to the caller as `UpdateMessage::Refused`.
    Refused(UpdateRefusal),
    /// `Bye`, a closed pipe or a protocol violation.
    CallerLeft,
}

/// Sends `SendInstaller`, then reads `InstallerChunk`s into `sink` through `stager`, sending
/// `Received` every `progress_every` bytes (design m5b D.4 steps 13–14).
///
/// Each chunk may take `chunk_wait`, all of them together `total`; a caller that stays silent
/// that long is refused with `CallerLeft`. Anything but a chunk (a `Bye`, a closed pipe, a request
/// or another update message in the middle of the transfer) ends it as `CallerLeft`; a message out
/// of order is answered with a protocol error first.
pub fn receive_installer(
    link: &mut dyn StageLink,
    sink: &mut dyn StageSink,
    mut stager: Stager,
    chunk_wait: Duration,
    total: Duration,
    progress_every: u64,
) -> StagingEnd {
    let asset = stager.plan().asset.clone();
    let offer = UpdateMessage::SendInstaller {
        name: asset.name.clone(),
        size: asset.size,
        sha256: asset.sha256.to_hex(),
        chunk_len: u32::try_from(CHUNK_LEN).unwrap_or(u32::MAX),
    };
    if link.send(HelperMessage::Update(offer)).is_err() {
        return StagingEnd::CallerLeft;
    }
    let started = Instant::now();
    let progress_every = progress_every.max(1);
    while !stager.is_complete() {
        let remaining = total.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return refuse_transfer(link, UpdateRefusal::CallerLeft);
        }
        let chunk = match link.recv(chunk_wait.min(remaining)) {
            Ok(CallerMessage::InstallerChunk(chunk)) => chunk,
            Ok(CallerMessage::Bye) => return StagingEnd::CallerLeft,
            Ok(other) => {
                let _ = link.send(HelperMessage::Error(protocol_error(&format!(
                    "{} during the installer transfer",
                    message_name(&other)
                ))));
                return StagingEnd::CallerLeft;
            }
            Err(FrameError::Timeout) => {
                return refuse_transfer(link, UpdateRefusal::CallerLeft);
            }
            Err(_) => return StagingEnd::CallerLeft,
        };
        let bytes = match chunk.decode() {
            Ok(bytes) => bytes,
            Err(refusal) => return refuse_transfer(link, refusal),
        };
        let before = stager.received();
        let received = match stager.accept(chunk.offset, &bytes) {
            Ok(received) => received,
            Err(refusal) => return refuse_transfer(link, refusal),
        };
        if let Err(detail) = sink.write_all(&bytes) {
            return refuse_transfer(link, UpdateRefusal::Storage { detail });
        }
        if received / progress_every > before / progress_every {
            let progress = UpdateMessage::Received { bytes: received };
            if link.send(HelperMessage::Update(progress)).is_err() {
                return StagingEnd::CallerLeft;
            }
        }
    }
    match stager.finish() {
        Ok(digest) => StagingEnd::Complete(digest),
        Err(refusal) => refuse_transfer(link, refusal),
    }
}

/// Sends `Refused(refusal)` during the transfer.
fn refuse_transfer(link: &mut dyn StageLink, refusal: UpdateRefusal) -> StagingEnd {
    match link.send(HelperMessage::Update(UpdateMessage::Refused(
        refusal.clone(),
    ))) {
        Ok(()) => StagingEnd::Refused(refusal),
        Err(_) => StagingEnd::CallerLeft,
    }
}

/// The wire name of a caller message, for protocol errors.
fn message_name(message: &CallerMessage) -> &'static str {
    match message {
        CallerMessage::Welcome(_) => "welcome",
        CallerMessage::Request(_) => "request",
        CallerMessage::Decision(_) => "decision",
        CallerMessage::Bye => "bye",
        CallerMessage::RecordTrust(_) => "record-trust",
        CallerMessage::StageUpdate(_) => "stage-update",
        CallerMessage::InstallerChunk(_) => "installer-chunk",
    }
}

/// A protocol violation, as the helper reports it before it disconnects.
pub fn protocol_error(message: &str) -> ErrorInfo {
    ErrorInfo {
        code: ErrorCode::Protocol,
        message: message.to_string(),
        op_id: None,
        plan_error: None,
    }
}

/// Everything H1 does to the machine (design m5b D.4). The helper implements it over mklm-win
/// (`apps/mklm-helper/src/update.rs`); tests use fakes (OPS-UX-TEST-10).
pub trait StagerEnv {
    fn version(&self) -> &Version;
    fn arch(&self) -> Arch;
    fn anchors(&self) -> &TrustAnchors;
    fn me(&self) -> ProcessIdentity;
    fn boot_id(&self) -> BootId;
    fn now(&self) -> Timestamp;
    fn now_unix(&self) -> u64;
    /// Step 1.
    fn runs_from_install_dir(&mut self) -> bool;
    /// The pipe server with its creation time, and its session.
    fn caller(&mut self) -> (Option<ProcessIdentity>, Option<u32>);
    fn free_space_program_data(&mut self) -> Result<u64, String>;
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal>;
    fn release_lock(&mut self);
    fn read_journal(&mut self) -> Result<Journal, String>;
    fn liveness(&self, process: &ProcessIdentity) -> Liveness;
    fn read_install_state(&mut self) -> InstallState;
    /// A corrupted value reads as empty (logged).
    fn read_trust(&mut self) -> TrustState;
    fn write_trust(&mut self, trust: &TrustState) -> Result<(), String>;
    fn read_run(&mut self) -> Result<Option<RunRecord>, String>;
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String>;
    fn delete_run(&mut self) -> Result<(), String>;
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String>;
    fn sweep_run_dirs(&mut self, keep: Option<&RunId>);
    fn random_suffix(&mut self) -> [u8; 8];
    fn create_run_dir(&mut self, run_id: &RunId) -> Result<(), String>;
    fn remove_run_dir(&mut self, run_id: &RunId);
    fn write_run_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String>;
    fn create_installer(&mut self, name: &str) -> Result<Box<dyn StagedInstaller>, String>;
    /// `$INSTDIR\mklm-helper.exe` → `<run dir>\RUNNER_EXE_NAME`, SHA-256 compared.
    fn copy_self_as_runner(&mut self) -> Result<(), String>;
    /// `<run dir>\tmp` with `PRIVATE_DIR_SDDL`.
    fn create_tmp_dir(&mut self) -> Result<(), String>;
    /// Clean environment, current directory System32 (SECURITY-6).
    fn spawn_runner(&mut self, run_id: &RunId) -> Result<ProcessIdentity, String>;
    /// `Some(exit code)` once the runner has exited.
    fn runner_exit_code(&mut self) -> Option<u32>;
    /// `TerminateProcess`, then waits for its handle up to `wait` (RELIABILITY-5).
    fn stop_runner(&mut self, wait: Duration);
    fn sleep(&mut self, duration: Duration);
    fn log(&mut self, line: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageFlowEnd {
    /// `HandedOff` was sent; the helper exits 0.
    HandedOff {
        run_id: RunId,
    },
    /// Already sent as `UpdateMessage::Refused`; the session goes on.
    Refused(UpdateRefusal),
    CallerLeft,
}

/// Design m5b D.4 steps 1–20 for one `StageUpdate`. The helper sets its heartbeat flag around
/// the call (D.3).
///
/// Nothing is changed when it is refused before step 9, and only the machine record `Trust` (a
/// verified manifest's `issued_at` and revocations) from step 9 on; a run folder and `Run` that
/// were made are removed again on every failure, and no `LastResult` is written (the caller sees
/// the answer, or left). The lock is held from step 5 until the end.
pub fn stage_update(
    env: &mut dyn StagerEnv,
    link: &mut dyn StageLink,
    request: &StageUpdateRequest,
) -> StageFlowEnd {
    let started = env.now();
    env.log("stage: a StageUpdate arrived");
    // Step 1: never a development copy (`target\…`) or a copy elsewhere.
    if !env.runs_from_install_dir() {
        return refuse(env, link, UpdateRefusal::NotInstalledCopy);
    }
    // Steps 2–3.
    let trust = env.read_trust();
    let verified = match verify(env, request, &trust) {
        Ok(verified) => verified,
        Err(refusal) => return refuse(env, link, refusal),
    };
    // Step 4: the installer and some room besides.
    let needed = verified.asset.size.saturating_add(space::STAGING_MARGIN);
    match env.free_space_program_data() {
        Ok(available) if available >= needed => {}
        Ok(available) => return refuse(env, link, UpdateRefusal::DiskFull { needed, available }),
        Err(detail) => return refuse(env, link, UpdateRefusal::Storage { detail }),
    }
    // Step 5.
    if let Err(refusal) = env.acquire_lock(timing::LOCK_WAIT_STAGER) {
        return refuse(env, link, refusal);
    }
    let end = stage_locked(env, link, request, verified, &trust, started);
    env.release_lock();
    end
}

/// Steps 2–3: `verify_manifest` for `Install` with this build's version, architecture and keys.
fn verify(
    env: &mut dyn StagerEnv,
    request: &StageUpdateRequest,
    trust: &TrustState,
) -> Result<VerifiedManifest, UpdateRefusal> {
    let now_unix = env.now_unix();
    verify_manifest(&VerifyInput {
        manifest: request.manifest.as_bytes(),
        signature: request.signature.as_bytes(),
        anchors: env.anchors(),
        state: trust,
        installed: env.version(),
        arch: env.arch(),
        now_unix,
        tag: None,
        purpose: Purpose::Install,
    })
}

/// Steps 6–20, under the lock (the caller releases it).
fn stage_locked(
    env: &mut dyn StagerEnv,
    link: &mut dyn StageLink,
    request: &StageUpdateRequest,
    mut verified: VerifiedManifest,
    trust_before: &TrustState,
    started: Timestamp,
) -> StageFlowEnd {
    // Step 6.
    let journal = match env.read_journal() {
        Ok(journal) => journal,
        Err(detail) => {
            env.log(&format!("stage: the journal cannot be read: {detail}"));
            return refuse(env, link, UpdateRefusal::JournalUnreadable);
        }
    };
    if let Err(refusal) = check_journal(&journal) {
        return refuse(env, link, refusal);
    }
    // Step 7.
    let run = match env.read_run() {
        Ok(run) => run,
        Err(detail) => return refuse(env, link, UpdateRefusal::Storage { detail }),
    };
    let boot = env.boot_id();
    let view = {
        let liveness = |process: &ProcessIdentity| env.liveness(process);
        classify_run(run.as_ref(), boot, &liveness)
    };
    match view {
        RunView::InProgress { .. } => return refuse(env, link, UpdateRefusal::UpdateInProgress),
        RunView::Interrupted(record) => {
            env.log(&format!(
                "stage: run {} was interrupted in {:?}",
                record.run_id.as_str(),
                record.phase
            ));
            let after = env.read_install_state();
            let result = interrupted_result(&record, env.now(), &after);
            if let Err(detail) = env
                .write_last_result(&result)
                .and_then(|()| env.delete_run())
            {
                return refuse(env, link, UpdateRefusal::Storage { detail });
            }
        }
        RunView::Idle => {}
    }
    // Step 8: nothing points at any run folder now.
    env.sweep_run_dirs(None);
    // Step 9: the machine record, read again under the lock. When it moved since the check of
    // step 3 (a RecordTrust of another session), the manifest is verified against it again.
    let trust = env.read_trust();
    if trust != *trust_before {
        verified = match verify(env, request, &trust) {
            Ok(verified) => verified,
            Err(refusal) => return refuse(env, link, refusal),
        };
    }
    let recorded = trust.recorded(&verified, env.now_unix());
    if recorded != trust
        && let Err(detail) = env.write_trust(&recorded)
    {
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    // Step 10.
    let run_id = RunId::new(&verified.version, env.random_suffix());
    if let Err(detail) = env.create_run_dir(&run_id) {
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    // Step 11.
    let (caller, caller_session) = env.caller();
    let now = env.now();
    let mut record = RunRecord {
        schema: RECORD_SCHEMA,
        run_id: run_id.clone(),
        from_version: env.version().to_string(),
        to_version: verified.version.to_string(),
        arch: env.arch(),
        phase: RunPhase::Staging,
        boot_id: boot,
        started_at: started,
        phase_at: now,
        caller,
        caller_session,
        stager: env.me(),
        runner: None,
        installer: None,
    };
    if let Err(detail) = env.write_run(&record) {
        env.remove_run_dir(&run_id);
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    env.log(&format!("stage: run {} staging", run_id.as_str()));
    let end = stage_run(env, link, request, &verified, &mut record);
    match end {
        StageFlowEnd::HandedOff { .. } => {}
        StageFlowEnd::Refused(_) | StageFlowEnd::CallerLeft => {
            env.remove_run_dir(&run_id);
            if let Err(detail) = env.delete_run() {
                env.log(&format!("stage: Run cannot be deleted: {detail}"));
            }
        }
    }
    end
}

/// Steps 12–20 with the run folder and `Run` made. On a failure the caller removes both.
fn stage_run(
    env: &mut dyn StagerEnv,
    link: &mut dyn StageLink,
    request: &StageUpdateRequest,
    verified: &VerifiedManifest,
    record: &mut RunRecord,
) -> StageFlowEnd {
    let run_id = record.run_id.clone();
    // Step 12: exactly the bytes that verified; H2 verifies them again.
    for (name, bytes) in [
        (MANIFEST_NAME, request.manifest.as_bytes()),
        (SIGNATURE_NAME, request.signature.as_bytes()),
    ] {
        if let Err(detail) = env.write_run_file(name, bytes) {
            return refuse(env, link, UpdateRefusal::Storage { detail });
        }
    }
    // Steps 13–14.
    let mut installer = match env.create_installer(&verified.asset.name) {
        Ok(installer) => installer,
        Err(detail) => return refuse(env, link, UpdateRefusal::Storage { detail }),
    };
    let stager = Stager::new(StagePlan::new(verified, env.version(), run_id.clone()));
    match receive_installer(
        link,
        installer.as_mut(),
        stager,
        timing::CHUNK_WAIT,
        timing::STAGE_TOTAL,
        PROGRESS_EVERY,
    ) {
        StagingEnd::Complete(_) => {}
        StagingEnd::Refused(refusal) => {
            env.log(&format!("stage: the installer was refused: {refusal}"));
            return StageFlowEnd::Refused(refusal);
        }
        StagingEnd::CallerLeft => {
            env.log("stage: the caller left during the installer transfer");
            return StageFlowEnd::CallerLeft;
        }
    }
    if let Err(detail) = installer.commit() {
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    // Step 15.
    if let Err(detail) = env.copy_self_as_runner() {
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    // Step 16.
    if let Err(detail) = env.create_tmp_dir() {
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    // Step 17.
    record.phase = RunPhase::Staged;
    record.phase_at = env.now();
    if let Err(detail) = env.write_run(record) {
        return refuse(env, link, UpdateRefusal::Storage { detail });
    }
    // Step 18.
    if link
        .send(HelperMessage::Update(UpdateMessage::StartingRunner))
        .is_err()
    {
        env.log("stage: the caller left before the runner started");
        return StageFlowEnd::CallerLeft;
    }
    let runner = match env.spawn_runner(&run_id) {
        Ok(runner) => runner,
        Err(detail) => return refuse(env, link, UpdateRefusal::HandOffFailed { detail }),
    };
    env.log(&format!("stage: runner process {} started", runner.pid));
    // Step 19.
    if let Err(detail) = wait_until_ready(env, &run_id) {
        return refuse(env, link, UpdateRefusal::HandOffFailed { detail });
    }
    // Step 20. The runner owns the run now, whatever happens to this message.
    env.log(&format!("stage: run {} handed off", run_id.as_str()));
    let handed_off = UpdateMessage::HandedOff {
        run_id: run_id.as_str().to_string(),
        to_version: record.to_version.clone(),
    };
    if link.send(HelperMessage::Update(handed_off)).is_err() {
        env.log("stage: the caller left before HandedOff; the runner goes on");
    }
    StageFlowEnd::HandedOff { run_id }
}

/// Step 19: `Run.phase` reaches `ready` (or later: the runner took the run over and may already
/// have finished and deleted it) within `READY_WAIT`. A runner that ended before, or is still not
/// ready then, is stopped and waited for, so that its folder can be removed.
fn wait_until_ready(env: &mut dyn StagerEnv, run_id: &RunId) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    loop {
        match env.read_run() {
            Ok(Some(run)) if run.run_id == *run_id && run.phase != RunPhase::Staged => {
                if run.runner.is_some() {
                    return Ok(());
                }
            }
            // The runner deleted it: it got past `ready` and already ended its run.
            Ok(None) => return Ok(()),
            Ok(Some(_)) => {}
            Err(detail) => env.log(&format!("stage: Run cannot be read: {detail}")),
        }
        if let Some(code) = env.runner_exit_code() {
            // One last look: it may have written `ready` just before it ended.
            if let Ok(Some(run)) = env.read_run()
                && run.run_id == *run_id
                && run.phase != RunPhase::Staged
                && run.runner.is_some()
            {
                return Ok(());
            }
            return Err(format!(
                "the update runner exited with code {code} before it was ready"
            ));
        }
        if waited >= timing::READY_WAIT {
            env.stop_runner(timing::RUNNER_KILL_WAIT);
            return Err(format!(
                "the update runner was not ready within {} s",
                timing::READY_WAIT.as_secs()
            ));
        }
        env.sleep(READY_POLL);
        waited += READY_POLL;
    }
}

/// Sends `Refused(refusal)`; the session goes on (`CallerLeft` when it cannot be sent).
fn refuse(
    env: &mut dyn StagerEnv,
    link: &mut dyn StageLink,
    refusal: UpdateRefusal,
) -> StageFlowEnd {
    env.log(&format!("stage: refused: {refusal}"));
    match link.send(HelperMessage::Update(UpdateMessage::Refused(
        refusal.clone(),
    ))) {
        Ok(()) => StageFlowEnd::Refused(refusal),
        Err(_) => StageFlowEnd::CallerLeft,
    }
}

/// The helper's handling of `CallerMessage::RecordTrust` (design m5b C.4; SECURITY-5,
/// FIX-VERIFICATION-6): `acquire_lock(TRUST_LOCK_WAIT)` (else `TrustNotRecorded(Busy)`), reads
/// the machine record again, `apply_trust_report` against it with `env.anchors()`,
/// `env.version()`, `env.arch()`, `env.now_unix()`, writes it when changed, releases the lock.
/// Returns the reply to send; the session goes on whatever it is.
pub fn record_trust(env: &mut dyn StagerEnv, report: &TrustReport) -> UpdateMessage {
    if let Err(refusal) = env.acquire_lock(timing::TRUST_LOCK_WAIT) {
        env.log(&format!("record-trust: no lock: {refusal}"));
        return UpdateMessage::TrustNotRecorded(refusal);
    }
    let machine = env.read_trust();
    let now_unix = env.now_unix();
    let applied = apply_trust_report(
        report.manifest.as_bytes(),
        report.signature.as_bytes(),
        env.anchors(),
        env.version(),
        env.arch(),
        &machine,
        now_unix,
    );
    let reply = match applied {
        Ok(Some(merged)) => match env.write_trust(&merged) {
            Ok(()) => UpdateMessage::TrustRecorded { changed: true },
            Err(detail) => UpdateMessage::TrustNotRecorded(UpdateRefusal::Storage { detail }),
        },
        Ok(None) => UpdateMessage::TrustRecorded { changed: false },
        Err(refusal) => UpdateMessage::TrustNotRecorded(refusal),
    };
    env.release_lock();
    env.log(&format!("record-trust: {reply:?}"));
    reply
}

/// The order rule of the caller's messages the helper session enforces (design m5b D.3):
/// `RecordTrust` only as the first message after `Welcome`. The helper's `serve` loop feeds every
/// message after `Welcome` through it; `Err` is a protocol violation (`HelperMessage::Error`,
/// then disconnect).
///
/// Also refused here: a second `Welcome`, and an `InstallerChunk` (chunks are only read by
/// [`receive_installer`] after its `SendInstaller`; the session loop never expects one).
#[derive(Debug, Default)]
pub struct CallerOrder {
    /// Messages seen after `Welcome`.
    seen: u64,
}

impl CallerOrder {
    pub fn new() -> CallerOrder {
        CallerOrder::default()
    }

    pub fn check(&mut self, message: &CallerMessage) -> Result<(), &'static str> {
        let first = self.seen == 0;
        self.seen = self.seen.saturating_add(1);
        match message {
            CallerMessage::RecordTrust(_) if first => Ok(()),
            CallerMessage::RecordTrust(_) => {
                Err("record-trust is only allowed as the first message after welcome")
            }
            CallerMessage::InstallerChunk(_) => Err("an installer chunk without send-installer"),
            CallerMessage::Welcome(_) => Err("a second welcome frame"),
            CallerMessage::Request(_)
            | CallerMessage::Decision(_)
            | CallerMessage::Bye
            | CallerMessage::StageUpdate(_) => Ok(()),
        }
    }
}
