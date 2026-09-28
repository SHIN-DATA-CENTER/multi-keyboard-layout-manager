//! The helper's side of an update session, as pure drivers over environment traits (design m5b
//! D.3, D.4, C.4; OPS-UX-TEST-10): H1's [`stage_update`] and [`receive_installer`], the
//! `RecordTrust` handling [`record_trust`], and the order rule of the caller's messages
//! [`CallerOrder`]. The helper implements the traits over mklm-win
//! (`apps/mklm-helper/src/update.rs`); tests use fakes.
//!
//! WP-0 writes the traits and types; WP-H the drivers.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::time::Duration;

use mklm_core::{BootId, Journal, Liveness, ProcessIdentity, Timestamp};
use mklm_update::run::{InstallState, RunId, RunRecord, UpdateResult};
use mklm_update::stage::Stager;
use mklm_update::{Arch, Sha256Digest, TrustAnchors, TrustState, Version};

use crate::frame::FrameError;
use crate::message::{CallerMessage, HelperMessage};
use crate::update::{StageUpdateRequest, TrustReport, UpdateMessage, UpdateRefusal};

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
pub fn receive_installer(
    link: &mut dyn StageLink,
    sink: &mut dyn StageSink,
    stager: Stager,
    chunk_wait: Duration,
    total: Duration,
    progress_every: u64,
) -> StagingEnd {
    StagingEnd::CallerLeft // Skeleton (M5b): WP-H
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
pub fn stage_update(
    env: &mut dyn StagerEnv,
    link: &mut dyn StageLink,
    request: &StageUpdateRequest,
) -> StageFlowEnd {
    // Skeleton (M5b): WP-H. Refuses without touching anything.
    let refusal = skeleton();
    env.log("stage_update: not implemented (m5b skeleton)");
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
    UpdateMessage::TrustNotRecorded(skeleton()) // Skeleton (M5b): WP-H
}

/// The order rule of the caller's messages the helper session enforces (design m5b D.3):
/// `RecordTrust` only as the first message after `Welcome`. The helper's `serve` loop feeds every
/// message after `Welcome` through it; `Err` is a protocol violation (`HelperMessage::Error`,
/// then disconnect).
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
        // Skeleton (M5b): WP-H. Until then the update messages are refused, the others pass.
        self.seen += 1;
        match message {
            CallerMessage::RecordTrust(_)
            | CallerMessage::StageUpdate(_)
            | CallerMessage::InstallerChunk(_) => Err("not implemented (m5b skeleton)"),
            CallerMessage::Welcome(_)
            | CallerMessage::Request(_)
            | CallerMessage::Decision(_)
            | CallerMessage::Bye => Ok(()),
        }
    }
}

/// What the WP-0 skeleton's unimplemented functions return (design m5b G.2).
fn skeleton() -> UpdateRefusal {
    UpdateRefusal::Internal {
        detail: "not implemented (m5b skeleton)".to_string(),
    }
}
