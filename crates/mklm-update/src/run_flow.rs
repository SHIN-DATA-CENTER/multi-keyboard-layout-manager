//! H2's steps (design m5b D.7) over [`RunnerEnv`], so that every failure can be injected in tests
//! (OPS-UX-TEST-10). The helper implements the environment over mklm-win
//! (`apps/mklm-helper/src/run_update.rs`).
//!
//! The order this driver fixes (design m5b F.2, G.6):
//! - nothing is recorded before `ready` (H1 reports a runner that ends early as `HandOffFailed`);
//! - the installer is created suspended, `Run = installing` with its identity is written, and only
//!   then is it resumed (RELIABILITY-4); a failed write terminates the never-run installer;
//! - at the end: `LastResult`, then `Run` deleted, then the lock released, then the clean-up, and
//!   the GUI relaunch last (D.10, D.11);
//! - an installer still running after `INSTALLER_WAIT` is never stopped: `LastResult =
//!   Failed(InstallerTimedOut)` is written and the wait goes on up to `INSTALLER_WAIT_MAX`, after
//!   which `Run` stays behind and nothing is relaunched.

use std::time::Duration;

use mklm_core::{BootId, Journal, ProcessIdentity, Timestamp};

use crate::gate::check_journal;
use crate::keys::TrustAnchors;
use crate::manifest::{Arch, Sha256Digest};
use crate::refusal::UpdateRefusal;
use crate::run::{
    FailedReason, FileHolder, InstallState, InstanceAnswer, NotInstalledReason, ProgramKind,
    RECORD_SCHEMA, RunId, RunPhase, RunRecord, RunningProgram, UpdateOutcome, UpdateResult,
    decide_outcome, space, timing,
};
use crate::state::TrustState;
use crate::verify::{Purpose, VerifyInput, verify_manifest};
use crate::{MAX_INSTALLER_LEN, Version, installer_name};

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

/// `--run-update` exit code: the update is installed (design m5b D.15).
pub const EXIT_INSTALLED: u32 = 0;
/// `--run-update` exit code: not installed or failed; the reason is in `LastResult` (none before
/// `ready`), or the installer outlived `INSTALLER_WAIT_MAX` (`Run` stays).
pub const EXIT_NOT_INSTALLED: u32 = 7;

/// Design m5b D.7 steps 2–22 (step 1 is the caller's). Returns the process exit code (0 / 7).
pub fn run_update(env: &mut dyn RunnerEnv, run_id: &RunId) -> u32 {
    env.log(&format!("run {}: runner started", run_id.as_str()));
    let Some(record) = prepare(env, run_id) else {
        // Nothing recorded: H1 sees this process end before `ready` (HandOffFailed).
        return EXIT_NOT_INSTALLED;
    };
    let from = env.version().clone();
    let to = run_id.version();
    let mut run = Run {
        env,
        record,
        from,
        to,
        lock_held: false,
    };
    run.install()
}

/// Steps 2–6: who runs, the record, the installer handle, `ready`. `None` (logged) when this
/// process must end without recording anything.
fn prepare(env: &mut dyn RunnerEnv, run_id: &RunId) -> Option<RunRecord> {
    // Step 2.
    match env.is_runner_of(run_id) {
        Ok(true) => {}
        Ok(false) => {
            env.log("this image is not the runner of the run folder: exit without a record");
            return None;
        }
        Err(detail) => {
            env.log(&format!(
                "the run folder cannot be verified ({detail}): exit without a record"
            ));
            return None;
        }
    }
    // Step 3: someone must have staged exactly this run for this build.
    let mut record = match env.read_run() {
        Ok(Some(record)) => record,
        Ok(None) => {
            env.log("no Run record: exit without a record");
            return None;
        }
        Err(detail) => {
            env.log(&format!(
                "the Run record cannot be read ({detail}): exit without a record"
            ));
            return None;
        }
    };
    let to = run_id.version();
    let expected = record.run_id == *run_id
        && record.phase == RunPhase::Staged
        && record.runner.is_none()
        && record.installer.is_none()
        && record.to_version == to.to_string()
        && record.from_version == env.version().to_string()
        && record.arch == env.arch();
    if !expected {
        env.log(&format!(
            "the Run record is not a staged run {} of this build ({} {:?}): exit without a record",
            run_id.as_str(),
            record.run_id.as_str(),
            record.phase
        ));
        return None;
    }
    // Step 4: held until the installer process exists.
    let name = installer_name(&to, env.arch());
    if let Err(detail) = env.lock_installer(&name, MAX_INSTALLER_LEN) {
        env.log(&format!(
            "{name} cannot be opened ({detail}): exit without a record"
        ));
        return None;
    }
    // Step 5, the session-end window, belongs to the environment (it exists before this runs).
    // Step 6.
    record.phase = RunPhase::Ready;
    record.runner = Some(env.me());
    record.phase_at = env.now();
    if let Err(detail) = env.write_run(&record) {
        env.log(&format!(
            "Run = ready cannot be written ({detail}): exit without a record"
        ));
        return None;
    }
    env.log("ready");
    Some(record)
}

/// One run from `ready` on (steps 7–22).
struct Run<'a> {
    env: &'a mut dyn RunnerEnv,
    record: RunRecord,
    from: Version,
    to: Version,
    lock_held: bool,
}

impl Run<'_> {
    fn install(&mut self) -> u32 {
        // Step 8: H1 has let go of the lock and of the pipe by the time it exits.
        let stager = self.record.stager;
        if !self.env.wait_for_exit(&stager, timing::STAGER_EXIT_WAIT) {
            self.env
                .log("the stager still runs after 30 s; the lock wait below decides");
        }
        let asset = match self.verify_again() {
            Ok(asset) => asset,
            Err(refusal) => return self.not_installed(NotInstalledReason::Refused(refusal)),
        };
        // Step 9.
        match self.env.acquire_lock(timing::LOCK_WAIT_RUNNER) {
            Ok(()) => self.lock_held = true,
            Err(refusal) => return self.not_installed(NotInstalledReason::Refused(refusal)),
        }
        // Step 10.
        let journal = match self.env.read_journal() {
            Ok(journal) => journal,
            Err(detail) => {
                self.env
                    .log(&format!("the journal cannot be read: {detail}"));
                return self.not_installed(NotInstalledReason::Refused(
                    UpdateRefusal::JournalUnreadable,
                ));
            }
        };
        if let Err(refusal) = check_journal(&journal) {
            return self.not_installed(NotInstalledReason::Refused(refusal));
        }
        // Step 11: the installed copy is still the one this runner was copied from.
        let build_id = self.env.build_id().to_string();
        match self.env.installed_helper_build_id() {
            Ok(Some(found)) if found == build_id => {}
            Ok(found) => {
                return self.not_installed(NotInstalledReason::InstalledVersionChanged { found });
            }
            Err(detail) => {
                self.env.log(&format!(
                    "the installed helper's build ID cannot be read: {detail}"
                ));
                return self
                    .not_installed(NotInstalledReason::InstalledVersionChanged { found: None });
            }
        }
        // Step 12: the new files next to the old ones, from the compressed installer's size.
        let needed = asset
            .0
            .saturating_mul(space::INSTALL_FACTOR)
            .saturating_add(space::INSTALL_MARGIN);
        match self.env.free_space_program_files() {
            Ok(available) if available >= needed => {}
            Ok(available) => {
                return self.not_installed(NotInstalledReason::DiskFull { needed, available });
            }
            Err(detail) => {
                return self.not_installed(NotInstalledReason::Refused(UpdateRefusal::Storage {
                    detail,
                }));
            }
        }
        // Step 13.
        if let Err(detail) = self.set_phase(RunPhase::Waiting) {
            return self.not_installed(NotInstalledReason::Refused(UpdateRefusal::Storage {
                detail,
            }));
        }
        if let Err(reason) = self.other_programs_gone() {
            return self.not_installed(reason);
        }
        // Step 14.
        let (programs, holders) = self.env.files_in_use();
        if !programs.is_empty() {
            return self.not_installed(NotInstalledReason::FilesInUse { programs, holders });
        }
        // Steps 15–16.
        if let Err(reason) = self.start_installer() {
            return self.not_installed(reason);
        }
        // Step 17.
        let Some(code) = self.wait_installer() else {
            // Still running after INSTALLER_WAIT_MAX: `Run` stays (`installing` with the
            // installer), and whoever looks after it has ended decides from the files.
            self.env.set_block_reason(false);
            self.env.release_installing();
            self.release_lock();
            self.env
                .log("the installer still runs after 60 minutes: Run stays, no relaunch, exit 7");
            return EXIT_NOT_INSTALLED;
        };
        // Step 18.
        self.env.set_block_reason(false);
        self.env
            .log(&format!("the installer exited with code {code}"));
        if let Err(detail) = self.set_phase(RunPhase::Finishing) {
            self.env
                .log(&format!("Run = finishing cannot be written: {detail}"));
        }
        self.env.release_installing();
        let after = self.env.read_install_state();
        let outcome = decide_outcome(&self.from, &self.to, Some(code), false, &after);
        // Steps 19–22.
        self.conclude(outcome, Some(code), &after, true)
    }

    /// Step 8: the staged manifest and signature against the machine record and this build's
    /// keys, the version of the run, and the installer's size and SHA-256 through the handle of
    /// step 4. Returns the verified size and digest.
    fn verify_again(&mut self) -> Result<(u64, Sha256Digest), UpdateRefusal> {
        let (manifest, signature) = self
            .env
            .read_staged_manifest()
            .map_err(|detail| UpdateRefusal::Storage { detail })?;
        let trust = self.env.read_trust();
        let now_unix = self.env.now_unix();
        let verified = verify_manifest(&VerifyInput {
            manifest: &manifest,
            signature: &signature,
            anchors: self.env.anchors(),
            state: &trust,
            installed: self.env.version(),
            arch: self.env.arch(),
            now_unix,
            tag: None,
            purpose: Purpose::Install,
        })?;
        if verified.version != self.to {
            return Err(UpdateRefusal::Internal {
                detail: format!(
                    "the staged manifest is for {}, the run for {}",
                    verified.version, self.to
                ),
            });
        }
        let (len, digest) = self
            .env
            .hash_locked_installer()
            .map_err(|detail| UpdateRefusal::Storage { detail })?;
        if len != verified.asset.size {
            return Err(UpdateRefusal::InstallerSizeMismatch {
                expected: verified.asset.size,
                received: len,
            });
        }
        if digest != verified.asset.sha256 {
            return Err(UpdateRefusal::InstallerHashMismatch);
        }
        self.env.log("verified again");
        Ok((verified.asset.size, verified.asset.sha256))
    }

    /// Design m5b D.8 steps 1–3.
    fn other_programs_gone(&mut self) -> Result<(), NotInstalledReason> {
        // 1. The GUI that asked quits on `HandedOff`.
        if let Some(caller) = self.record.caller
            && !self.env.wait_for_exit(&caller, timing::CALLER_EXIT_WAIT)
        {
            return Err(NotInstalledReason::CallerDidNotExit);
        }
        // 2. Every session's idle GUI: `quit-if-idle` only.
        let answers = self.env.quit_idle_instances();
        let mut busy = Vec::new();
        for (answer, session) in &answers {
            self.env
                .log(&format!("instance of session {session}: {answer:?}"));
            if *answer == InstanceAnswer::Busy && !busy.contains(session) {
                busy.push(*session);
            }
        }
        if !busy.is_empty() {
            return Err(NotInstalledReason::InstanceBusy { sessions: busy });
        }
        // 3. Every installed program: GUI and CLI 30 s, a helper up to 75 s. Once one does not
        //    end in time, the others are not waited for (the result is the same).
        for program in self.env.running_programs() {
            let wait = match program.kind {
                ProgramKind::Helper => timing::HELPERS_EXIT_WAIT,
                ProgramKind::Gui | ProgramKind::Cli => timing::PROGRAMS_EXIT_WAIT,
            };
            if !self.env.wait_for_exit(&program.identity, wait) {
                break;
            }
        }
        let still = self.env.running_programs();
        if still.is_empty() {
            return Ok(());
        }
        let mut programs = Vec::new();
        for program in &still {
            if !programs.contains(&program.kind) {
                programs.push(program.kind);
            }
        }
        let holders = still
            .iter()
            .map(|program| FileHolder {
                pid: program.identity.pid,
                session_id: program.session_id,
                name: program_file_name(program.kind).to_string(),
            })
            .collect();
        Err(NotInstalledReason::ProgramsStillRunning { programs, holders })
    }

    /// Steps 15–16: the session-end check and the suspended installer, recorded before it runs.
    fn start_installer(&mut self) -> Result<(), NotInstalledReason> {
        if !self.env.claim_installing() {
            return Err(NotInstalledReason::SessionEnding);
        }
        let installer = match self.env.spawn_installer_suspended() {
            Ok(installer) => installer,
            Err(code) => {
                self.env.release_installing();
                return Err(NotInstalledReason::InstallerNotStarted { code });
            }
        };
        self.record.phase = RunPhase::Installing;
        self.record.installer = Some(installer);
        self.record.phase_at = self.env.now();
        if let Err(detail) = self.env.write_run(&self.record) {
            // It never ran: nothing may run that the record does not know of.
            self.env.terminate_suspended_installer();
            self.env.release_installing();
            self.record.installer = None;
            return Err(NotInstalledReason::Refused(UpdateRefusal::Storage {
                detail,
            }));
        }
        self.env.set_block_reason(true);
        if let Err(detail) = self.env.resume_installer() {
            self.env.terminate_suspended_installer();
            self.env.set_block_reason(false);
            self.env.release_installing();
            return Err(NotInstalledReason::Refused(UpdateRefusal::Internal {
                detail: format!("the installer could not be resumed: {detail}"),
            }));
        }
        self.env
            .log(&format!("installing (installer process {})", installer.pid));
        Ok(())
    }

    /// Step 17: up to `INSTALLER_WAIT`, then `Failed(InstallerTimedOut)` is recorded and the wait
    /// goes on up to `INSTALLER_WAIT_MAX` in all. `None` when it is still running then.
    fn wait_installer(&mut self) -> Option<u32> {
        if let Some(code) = self.env.wait_installer(timing::INSTALLER_WAIT) {
            return Some(code);
        }
        self.env
            .log("the installer still runs after 15 minutes: recorded as timed out, waiting on");
        let after = self.env.read_install_state();
        let result = self.result(
            UpdateOutcome::Failed(FailedReason::InstallerTimedOut),
            None,
            &after,
            false,
        );
        if let Err(detail) = self.env.write_last_result(&result) {
            self.env
                .log(&format!("LastResult cannot be written: {detail}"));
        }
        self.env
            .wait_installer(timing::INSTALLER_WAIT_MAX.saturating_sub(timing::INSTALLER_WAIT))
    }

    fn set_phase(&mut self, phase: RunPhase) -> Result<(), String> {
        self.record.phase = phase;
        self.record.phase_at = self.env.now();
        self.env.write_run(&self.record)
    }

    fn release_lock(&mut self) {
        if self.lock_held {
            self.env.release_lock();
            self.lock_held = false;
        }
    }

    fn result(
        &self,
        outcome: UpdateOutcome,
        installer_exit: Option<u32>,
        after: &InstallState,
        gui_relaunch_attempted: bool,
    ) -> UpdateResult {
        UpdateResult {
            schema: RECORD_SCHEMA,
            run_id: self.record.run_id.clone(),
            from_version: self.record.from_version.clone(),
            to_version: self.record.to_version.clone(),
            arch: self.record.arch,
            finished_at: self.env.now(),
            outcome,
            installer_exit,
            installed_version: after
                .consistent_version()
                .map(|version| version.to_string()),
            gui_relaunch_attempted,
        }
    }

    /// Step 7: every failure after `ready`. The GUI is relaunched, except when the session is
    /// ending (it would not outlive the sign-out, and the RunOnce value brings it back).
    fn not_installed(&mut self, reason: NotInstalledReason) -> u32 {
        self.env.log(&format!("not installed: {reason:?}"));
        let relaunch = reason != NotInstalledReason::SessionEnding;
        let after = self.env.read_install_state();
        self.conclude(UpdateOutcome::NotInstalled(reason), None, &after, relaunch)
    }

    /// Steps 19–22: `LastResult`, `Run` deleted, the lock released, the clean-up, and the GUI
    /// relaunch last. When `LastResult` cannot be written, `Run` stays: the next start then
    /// reports the run as interrupted instead of reporting nothing.
    ///
    /// The GUI is relaunched only when `Run` is gone (MECHANICS-2). While `Run` still names this
    /// runner, alive until it exits a moment later, a GUI started in the caller's session sees an
    /// update in progress and quits at once without a window (D.13 step 1): the relaunch would
    /// leave the user with no MKLM at all. Without it the user opens MKLM (the hand-off told them
    /// to after 2 minutes, E.4), or the caller's RunOnce value and the Run key start it at the
    /// next sign-in; by then this runner is gone and the start reports the run as interrupted.
    /// `LastResult`, when it was written, is corrected to `gui_relaunch_attempted = false`.
    fn conclude(
        &mut self,
        outcome: UpdateOutcome,
        installer_exit: Option<u32>,
        after: &InstallState,
        relaunch: bool,
    ) -> u32 {
        let installed = outcome == UpdateOutcome::Installed;
        let mut result = self.result(outcome, installer_exit, after, relaunch);
        let (written, run_deleted) = match self.env.write_last_result(&result) {
            Ok(()) => match self.env.delete_run() {
                Ok(()) => (true, true),
                Err(detail) => {
                    self.env.log(&format!("Run cannot be deleted: {detail}"));
                    (true, false)
                }
            },
            Err(detail) => {
                self.env.log(&format!(
                    "LastResult cannot be written ({detail}); Run stays for the next start"
                ));
                (false, false)
            }
        };
        let skipped = relaunch && !run_deleted;
        if skipped && written {
            result.gui_relaunch_attempted = false;
            if let Err(detail) = self.env.write_last_result(&result) {
                self.env
                    .log(&format!("LastResult cannot be corrected: {detail}"));
            }
        }
        self.release_lock();
        self.env.cleanup_own_run_dir();
        self.env.sweep_other_run_dirs();
        if skipped {
            self.env.log(
                "the GUI is not relaunched: Run is still there, so it would quit at once; \
                 the RunOnce value and the Run key start it later",
            );
        } else if relaunch && let Err(detail) = self.env.relaunch_gui() {
            self.env
                .log(&format!("the GUI could not be relaunched: {detail}"));
        }
        self.env.log(&format!("finished: {:?}", result.outcome));
        if installed {
            EXIT_INSTALLED
        } else {
            EXIT_NOT_INSTALLED
        }
    }
}

/// H2's answer to the end of the Windows session (design m5b D.7 "サインアウトとシャットダウン";
/// RELIABILITY-3), shared by the driver's thread (`RunnerEnv::claim_installing` /
/// `release_installing`) and the hidden window's thread (`WM_QUERYENDSESSION` /
/// `WM_ENDSESSION`). One lock orders the two decisions, so that the installer is never started
/// once the session end has been let through, and the session end is never let through while it
/// runs.
///
/// - `ready` / `waiting` (before the installer): a query marks the session as ending and allows
///   it; the driver then refuses to start the installer (`SessionEnding`). A cancelled end
///   (`WM_ENDSESSION` with `FALSE`: another application refused) clears the mark.
/// - `installing`: every query is blocked.
/// - `finishing` and later: allowed.
#[derive(Debug, Default)]
pub struct SessionEndGate {
    state: std::sync::Mutex<GateState>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum GateState {
    #[default]
    BeforeInstaller,
    Ending,
    Installing,
    AfterInstaller,
}

impl SessionEndGate {
    pub fn new() -> SessionEndGate {
        SessionEndGate::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, GateState> {
        // A panicking thread must not disable the answer to Windows.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `WM_QUERYENDSESSION`: true to block the end of the session (only while installing).
    pub fn query_end_session(&self) -> bool {
        let mut state = self.state();
        match *state {
            GateState::Installing => true,
            GateState::BeforeInstaller => {
                *state = GateState::Ending;
                false
            }
            GateState::Ending | GateState::AfterInstaller => false,
        }
    }

    /// `WM_ENDSESSION`: `ending` false means another application refused and the session goes
    /// on, so a mark set by the query is cleared.
    pub fn end_session(&self, ending: bool) {
        let mut state = self.state();
        if !ending && *state == GateState::Ending {
            *state = GateState::BeforeInstaller;
        }
    }

    /// The driver, right before it creates the installer: false once the session end began.
    pub fn claim_installing(&self) -> bool {
        let mut state = self.state();
        match *state {
            GateState::BeforeInstaller | GateState::Installing => {
                *state = GateState::Installing;
                true
            }
            GateState::Ending | GateState::AfterInstaller => false,
        }
    }

    /// The driver, once the installer has ended or was never started.
    pub fn release_installing(&self) {
        let mut state = self.state();
        if *state == GateState::Installing {
            *state = GateState::AfterInstaller;
        }
    }

    /// True while a session end that was let through is pending.
    pub fn session_ending(&self) -> bool {
        *self.state() == GateState::Ending
    }
}

/// The installed file of each program.
fn program_file_name(kind: ProgramKind) -> &'static str {
    match kind {
        ProgramKind::Gui => "mklm.exe",
        ProgramKind::Cli => "mklm-cli.exe",
        ProgramKind::Helper => "mklm-helper.exe",
    }
}

#[cfg(test)]
mod tests;
