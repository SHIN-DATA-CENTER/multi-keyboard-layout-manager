//! H2's steps (design m5b D.7) over [`RunnerEnv`], so that every failure can be injected in tests
//! (OPS-UX-TEST-10). The helper implements the environment over mklm-win
//! (`apps/mklm-helper/src/run_update.rs`).
//!
//! WP-0 writes the trait; WP-H the driver.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::time::Duration;

use mklm_core::{BootId, Journal, ProcessIdentity, Timestamp};

use crate::Version;
use crate::keys::TrustAnchors;
use crate::manifest::{Arch, Sha256Digest};
use crate::refusal::UpdateRefusal;
use crate::run::{
    FileHolder, InstallState, InstanceAnswer, ProgramKind, RunId, RunRecord, RunningProgram,
    UpdateResult,
};
use crate::state::TrustState;

/// Everything H2 does to the machine (design m5b D.7). The helper implements it over mklm-win
/// (`apps/mklm-helper/src/run_update.rs`); tests use fakes. Errors are English diagnostics.
pub trait RunnerEnv {
    // Who and where
    fn version(&self) -> &Version;
    fn arch(&self) -> Arch;
    fn build_id(&self) -> &str;
    fn anchors(&self) -> &TrustAnchors;
    fn me(&self) -> ProcessIdentity;
    fn boot_id(&self) -> BootId;
    fn now(&self) -> Timestamp;
    fn now_unix(&self) -> u64;
    /// Step 2: this image is `Updates\<run_id>\mklm-update-runner.exe`, every level verified and pinned.
    fn is_runner_of(&mut self, run_id: &RunId) -> Result<bool, String>;
    // Records (HKLM Update)
    fn read_run(&mut self) -> Result<Option<RunRecord>, String>;
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String>;
    fn delete_run(&mut self) -> Result<(), String>;
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String>;
    fn read_trust(&mut self) -> TrustState;
    // The run folder
    /// Step 4: `open_locked` (FILE_SHARE_READ only), kept until the installer is created.
    fn lock_installer(&mut self, name: &str, max_len: u64) -> Result<(), String>;
    fn read_staged_manifest(&mut self) -> Result<(Vec<u8>, Vec<u8>), String>;
    /// Through the locked handle.
    fn hash_locked_installer(&mut self) -> Result<(u64, Sha256Digest), String>;
    fn cleanup_own_run_dir(&mut self);
    fn sweep_other_run_dirs(&mut self);
    // Waits, lock, journal
    fn wait_for_exit(&mut self, process: &ProcessIdentity, timeout: Duration) -> bool;
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal>;
    fn release_lock(&mut self);
    fn read_journal(&mut self) -> Result<Journal, String>;
    // The installed copy
    fn installed_helper_build_id(&mut self) -> Result<Option<String>, String>;
    fn free_space_program_files(&mut self) -> Result<u64, String>;
    fn read_install_state(&mut self) -> InstallState;
    // Other MKLM programs (D.8)
    /// Each GUI's answer with the session of its pipe (FIX-VERIFICATION-5: names computed from
    /// the GUI processes, never enumerated).
    fn quit_idle_instances(&mut self) -> Vec<(InstanceAnswer, u32)>;
    fn running_programs(&mut self) -> Vec<RunningProgram>;
    /// Retried for `FILES_IN_USE_RETRY`; the programs whose file stays in use, and who holds them
    /// (Restart Manager; empty when unknown. RED-TEAM-3).
    fn files_in_use(&mut self) -> (Vec<ProgramKind>, Vec<FileHolder>);
    // Session end (RELIABILITY-3)
    /// Atomically: if the session end has not begun, answer later WM_QUERYENDSESSION with "block"
    /// and return true; else false.
    fn claim_installing(&mut self) -> bool;
    fn set_block_reason(&mut self, on: bool);
    fn release_installing(&mut self);
    // The installer (RELIABILITY-4, SECURITY-6)
    /// Suspended, clean environment, current directory System32. `Err(Win32 code)`.
    fn spawn_installer_suspended(&mut self) -> Result<ProcessIdentity, u32>;
    fn terminate_suspended_installer(&mut self);
    /// Closes the step-4 handle, then `ResumeThread`.
    fn resume_installer(&mut self) -> Result<(), String>;
    /// The exit code, or `None` when still running after `timeout`.
    fn wait_installer(&mut self, timeout: Duration) -> Option<u32>;
    // The end
    fn relaunch_gui(&mut self) -> Result<(), String>;
    fn log(&mut self, line: &str);
}

/// Design m5b D.7 steps 2–22 (step 1 is the caller's). Returns the process exit code (0 / 7).
pub fn run_update(env: &mut dyn RunnerEnv, run_id: &RunId) -> u32 {
    // Skeleton (M5b): WP-H. Nothing is read or written; "not installed" without a record, as
    // before `ready` (design m5b D.15).
    env.log("run_update: not implemented (m5b skeleton)");
    7
}
