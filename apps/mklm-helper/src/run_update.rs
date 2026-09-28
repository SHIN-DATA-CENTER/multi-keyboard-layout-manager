//! H2: `mklm-update-runner.exe --run-update <run-id>` (design m5b D.7): step 1 here (COM
//! security, shutdown order, working folder, elevation and OS checks), then
//! `mklm_update::run_flow::run_update` over a `RunnerEnv` implemented with mklm-win.
//!
//! The runner is H1's copy in a protected run folder, started with a minimal environment block.
//! It installs with the NSIS installer H1 received, started suspended with the same minimal
//! environment and recorded in `Run` before it is resumed; it never starts anything with its own
//! token afterwards: the GUI comes back through the desktop's Explorer (D.10).

#![cfg(windows)]

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mklm_core::{BootId, Journal, ProcessIdentity, Timestamp};
use mklm_ipc::RunUpdateArgs;
use mklm_update::run::{
    FileHolder, InstallState, InstanceAnswer, ProgramKind, RunId, RunRecord, RunningProgram,
    UpdateResult, timing,
};
use mklm_update::run_flow::{RunnerEnv, SessionEndGate, run_update};
use mklm_update::{
    Arch, GUI_AFTER_UPDATE_ARG, MANIFEST_NAME, MAX_MANIFEST_LEN, MAX_SIGNATURE_LEN,
    RUNNER_EXE_NAME, SIGNATURE_NAME, Sha256Digest, TrustAnchors, TrustState, UpdateRefusal,
    Version,
};
use mklm_win::elevation::{self, SpawnedProcess, runner_environment, spawn_clean};
use mklm_win::instance::{QuitAnswer, quit_idle_instances};
use mklm_win::proc_identity::{
    file_nt_path, process_image_nt_path, processes_with_images, wait_for_exit,
};
use mklm_win::protected_dir::{DataDir, ProtectedDir, ensure_protected_dir, open_protected_dir};
use mklm_win::session_end::{
    QueryAnswer, SessionEndEvent, SessionEndWindow, shut_down_first, user_ui_language,
};
use mklm_win::shell_launch::{init_com_for_runner, launch_via_shell};
use mklm_win::update_dir::{
    INSTALLED_EXECUTABLES, LockedFile, RunDir, file_holders, files_in_use, free_space,
    sweep_stale_run_dirs,
};

use crate::exit;
use crate::update::{Build, Machine, UpdateLog, hash_reader, now, now_unix, same_path};

/// Oldest Windows build MKLM supports (Windows 11 24H2), as for a pipe session.
const MIN_BUILD: u32 = 26100;
/// This build's ID (apps/build_id.rs), which the installed helper must still carry (D.7 step 11).
const BUILD_ID: &str = env!("MKLM_BUILD_ID");
/// `LANG_JAPANESE`, the primary language of a Japanese UI.
const LANG_JAPANESE: u16 = 0x11;

/// Runs the update and returns the process exit code (design m5b D.15).
pub fn run(args: &RunUpdateArgs) -> u8 {
    let log = UpdateLog::new("H2");
    // Step 1. COM security first, before any other COM call (SECURITY-8); fatal when it fails.
    let _com = match init_com_for_runner() {
        Ok(com) => com,
        Err(error) => {
            log.line(&format!("COM security cannot be set: {error}"));
            return exit::FAILURE;
        }
    };
    if let Err(error) = shut_down_first() {
        log.line(&format!("the shutdown order cannot be set: {error}"));
    }
    match elevation::is_elevated() {
        Ok(true) => {}
        Ok(false) => return exit::NOT_ELEVATED,
        Err(_) => return exit::FAILURE,
    }
    match mklm_win::read_os_info(&mut Vec::new()) {
        Ok(os) if os.build >= MIN_BUILD => {}
        Ok(_) => return exit::UNSUPPORTED_OS,
        Err(_) => return exit::FAILURE,
    }
    let build = match Build::current() {
        Ok(build) => build,
        Err(refusal) => {
            log.line(&format!("not runnable: {refusal}"));
            return exit::NOT_UPDATED;
        }
    };
    // Step 5 (the window exists before the driver starts): the session end is answered by the
    // gate the driver claims before it creates the installer (RELIABILITY-3).
    let gate = Arc::new(SessionEndGate::new());
    let answering = Arc::clone(&gate);
    let window = SessionEndWindow::spawn_with_answer(move |event| match event {
        SessionEndEvent::QueryEndSession => {
            if answering.query_end_session() {
                QueryAnswer::Block
            } else {
                QueryAnswer::Allow
            }
        }
        SessionEndEvent::EndSession { ending } => {
            answering.end_session(ending);
            QueryAnswer::Allow
        }
    });
    let window = match window {
        Ok(window) => Some(window),
        Err(error) => {
            log.line(&format!(
                "no session-end window ({error}); a sign-out is not blocked"
            ));
            None
        }
    };
    let mut env = Runner {
        build,
        run_id: args.run_id.clone(),
        machine: Machine::default(),
        updates: None,
        run_dir: None,
        installer_name: String::new(),
        locked: None,
        installer: None,
        gate,
        window: window.as_ref(),
        log: &log,
    };
    let code = run_update(&mut env, &args.run_id);
    drop(env);
    drop(window);
    u8::try_from(code).unwrap_or(exit::NOT_UPDATED)
}

/// The text Windows shows while the installer runs and the runner blocks a sign-out (design
/// m5b D.7 step 16). The helper has no message catalogue; Windows shows this one string only.
fn block_reason() -> &'static str {
    if user_ui_language() & 0x3ff == LANG_JAPANESE {
        "MKLM を更新しています。数秒お待ちください"
    } else {
        "MKLM is being updated. Please wait a few seconds."
    }
}

/// H2's machine (design m5b D.7).
struct Runner<'a> {
    build: Build,
    run_id: RunId,
    machine: Machine,
    updates: Option<ProtectedDir>,
    run_dir: Option<RunDir>,
    installer_name: String,
    /// Step 4's handle, kept until the installer process exists.
    locked: Option<LockedFile>,
    installer: Option<SpawnedProcess>,
    gate: Arc<SessionEndGate>,
    window: Option<&'a SessionEndWindow>,
    log: &'a UpdateLog,
}

impl Runner<'_> {
    fn run_dir(&self) -> Result<&RunDir, String> {
        self.run_dir
            .as_ref()
            .ok_or_else(|| "the run folder is not open".to_string())
    }

    fn installed(&self, name: &str) -> PathBuf {
        self.build.install_dir.join(name)
    }

    /// The installed executables that exist, with their NT paths.
    fn installed_programs(&self) -> Vec<(ProgramKind, String)> {
        let kinds = [ProgramKind::Gui, ProgramKind::Cli, ProgramKind::Helper];
        INSTALLED_EXECUTABLES
            .iter()
            .zip(kinds)
            .filter_map(|(name, kind)| {
                file_nt_path(&self.installed(name))
                    .ok()
                    .map(|path| (kind, path))
            })
            .collect()
    }

    fn read_locked(&self, name: &str, max_len: usize) -> Result<Vec<u8>, String> {
        let mut file = self
            .run_dir()?
            .open_locked(name, max_len as u64)
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("ReadFile: {error}"))?;
        Ok(bytes)
    }
}

impl RunnerEnv for Runner<'_> {
    fn version(&self) -> &Version {
        &self.build.version
    }
    fn arch(&self) -> Arch {
        self.build.arch
    }
    fn build_id(&self) -> &str {
        BUILD_ID
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
    /// Step 2: `Updates` and the run folder verified (nothing created or moved) and pinned, and
    /// this image is the runner in it.
    fn is_runner_of(&mut self, run_id: &RunId) -> Result<bool, String> {
        let updates = open_protected_dir(DataDir::Updates).map_err(|error| error.to_string())?;
        let run_dir = RunDir::open(&updates, run_id.as_str()).map_err(|error| error.to_string())?;
        let expected = file_nt_path(&run_dir.path().join(RUNNER_EXE_NAME))
            .map_err(|error| error.to_string())?;
        let own = process_image_nt_path(self.build.me.pid).map_err(|error| error.to_string())?;
        let matches = same_path(&own, &expected);
        self.updates = Some(updates);
        self.run_dir = Some(run_dir);
        Ok(matches)
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
    fn read_trust(&mut self) -> TrustState {
        self.machine.read_trust(self.log)
    }
    fn lock_installer(&mut self, name: &str, max_len: u64) -> Result<(), String> {
        let locked = self
            .run_dir()?
            .open_locked(name, max_len)
            .map_err(|error| error.to_string())?;
        self.installer_name = name.to_string();
        self.locked = Some(locked);
        Ok(())
    }
    fn read_staged_manifest(&mut self) -> Result<(Vec<u8>, Vec<u8>), String> {
        let manifest = self.read_locked(MANIFEST_NAME, MAX_MANIFEST_LEN)?;
        let signature = self.read_locked(SIGNATURE_NAME, MAX_SIGNATURE_LEN)?;
        Ok((manifest, signature))
    }
    fn hash_locked_installer(&mut self) -> Result<(u64, Sha256Digest), String> {
        let locked = self
            .locked
            .as_mut()
            .ok_or_else(|| "the installer is not open".to_string())?;
        locked.rewind().map_err(|error| error.to_string())?;
        let hashed = hash_reader(locked)?;
        locked.rewind().map_err(|error| error.to_string())?;
        Ok(hashed)
    }
    /// Step 20: the installer, the two manifests and `tmp`; the runner itself is running, so its
    /// folder stays for the next sweep (no removal at restart: D.11).
    fn cleanup_own_run_dir(&mut self) {
        self.locked = None;
        let Some(run_dir) = self.run_dir.as_ref() else {
            return;
        };
        for file in [self.installer_name.as_str(), MANIFEST_NAME, SIGNATURE_NAME] {
            if !file.is_empty()
                && let Err(error) = run_dir.remove_file(file)
            {
                self.log.line(&format!("{file} not removed: {error}"));
            }
        }
        if let Err(error) = run_dir.remove_private_subdir("tmp") {
            self.log.line(&format!("tmp not removed: {error}"));
        }
    }
    fn sweep_other_run_dirs(&mut self) {
        let updates = match self.updates.take() {
            Some(updates) => Ok(updates),
            None => ensure_protected_dir(DataDir::Updates),
        };
        match updates.and_then(|updates| {
            let removed = sweep_stale_run_dirs(&updates, Some(self.run_id.as_str()));
            self.updates = Some(updates);
            removed
        }) {
            Ok(removed) if !removed.is_empty() => {
                self.log
                    .line(&format!("removed old run folders {removed:?}"));
            }
            Ok(_) => {}
            Err(error) => self
                .log
                .line(&format!("old run folders not swept: {error}")),
        }
    }
    fn wait_for_exit(&mut self, process: &ProcessIdentity, timeout: Duration) -> bool {
        wait_for_exit(&[*process], timeout).unwrap_or(false)
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
    fn installed_helper_build_id(&mut self) -> Result<Option<String>, String> {
        let helper = self.installed("mklm-helper.exe");
        if !helper.exists() {
            return Ok(None);
        }
        elevation::file_build_id(&helper)
            .map(Some)
            .map_err(|error| error.to_string())
    }
    fn free_space_program_files(&mut self) -> Result<u64, String> {
        let dir = &self.build.install_dir;
        let volume = if dir.exists() {
            dir.as_path()
        } else {
            dir.parent().unwrap_or(dir)
        };
        free_space(volume).map_err(|error| error.to_string())
    }
    fn read_install_state(&mut self) -> InstallState {
        self.build.install_state()
    }
    fn quit_idle_instances(&mut self) -> Vec<(InstanceAnswer, u32)> {
        let Ok(gui) = file_nt_path(&self.installed("mklm.exe")) else {
            return Vec::new();
        };
        match quit_idle_instances(
            &gui,
            timing::INSTANCE_QUIT_WAIT,
            timing::INSTANCES_TOTAL,
            timing::MAX_INSTANCE_PIPES,
        ) {
            Ok(answers) => answers
                .into_iter()
                .map(|quit| {
                    let answer = match quit.answer {
                        QuitAnswer::Ok => InstanceAnswer::Quit,
                        QuitAnswer::Busy => InstanceAnswer::Busy,
                        QuitAnswer::NotOurs => InstanceAnswer::NotOurs,
                        QuitAnswer::NoAnswer => InstanceAnswer::NoAnswer,
                    };
                    (answer, quit.session_id)
                })
                .collect(),
            Err(error) => {
                self.log
                    .line(&format!("the running GUIs cannot be asked: {error}"));
                Vec::new()
            }
        }
    }
    fn running_programs(&mut self) -> Vec<RunningProgram> {
        let programs = self.installed_programs();
        let paths: Vec<String> = programs.iter().map(|(_, path)| path.clone()).collect();
        match processes_with_images(&paths) {
            Ok(found) => found
                .into_iter()
                .filter_map(|process| {
                    programs
                        .get(process.path_index)
                        .map(|(kind, _)| RunningProgram {
                            identity: process.identity,
                            kind: *kind,
                            session_id: process.session_id,
                        })
                })
                .collect(),
            Err(error) => {
                // The installer checks again by path (codes 22-24): nothing is lost.
                self.log
                    .line(&format!("the running programs cannot be listed: {error}"));
                Vec::new()
            }
        }
    }
    /// Design m5b D.8 step 4: retried for `FILES_IN_USE_RETRY` (antivirus software reads briefly).
    fn files_in_use(&mut self) -> (Vec<ProgramKind>, Vec<FileHolder>) {
        let kinds = [ProgramKind::Gui, ProgramKind::Cli, ProgramKind::Helper];
        let deadline = Instant::now() + timing::FILES_IN_USE_RETRY;
        let mut in_use;
        loop {
            in_use = match files_in_use(&self.build.install_dir, &INSTALLED_EXECUTABLES) {
                Ok(in_use) => in_use,
                Err(error) => {
                    // The installer's rename (code 26, rolled back) is the last check.
                    self.log
                        .line(&format!("files in use cannot be checked: {error}"));
                    Vec::new()
                }
            };
            if in_use.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let programs: Vec<ProgramKind> = in_use
            .iter()
            .filter_map(|&index| kinds.get(index).copied())
            .collect();
        if programs.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let paths: Vec<PathBuf> = in_use
            .iter()
            .filter_map(|&index| INSTALLED_EXECUTABLES.get(index))
            .map(|name| self.installed(name))
            .collect();
        let holders = file_holders(&paths)
            .map(|holders| {
                holders
                    .into_iter()
                    .map(|(pid, session_id, name)| FileHolder {
                        pid,
                        session_id,
                        name,
                    })
                    .collect()
            })
            .unwrap_or_else(|error| {
                self.log
                    .line(&format!("the holders cannot be found: {error}"));
                Vec::new()
            });
        (programs, holders)
    }
    fn claim_installing(&mut self) -> bool {
        self.gate.claim_installing()
    }
    fn set_block_reason(&mut self, on: bool) {
        if let Some(window) = self.window
            && let Err(error) = window.set_block_reason(on.then(block_reason))
        {
            self.log.line(&format!("block reason: {error}"));
        }
    }
    fn release_installing(&mut self) {
        self.gate.release_installing();
    }
    fn spawn_installer_suspended(&mut self) -> Result<ProcessIdentity, u32> {
        let code = |error: mklm_win::Error| error.win32_code().unwrap_or(0);
        let run_dir = self.run_dir().map_err(|_| 0u32)?;
        let exe = run_dir.path().join(&self.installer_name);
        let environment = runner_environment(&run_dir.path().join("tmp")).map_err(code)?;
        let installer = spawn_clean(&exe, "/S", &environment, true).map_err(code)?;
        let identity = match installer.identity() {
            Ok(identity) => identity,
            Err(error) => {
                let _ = installer.terminate(1);
                return Err(code(error));
            }
        };
        self.installer = Some(installer);
        Ok(identity)
    }
    fn terminate_suspended_installer(&mut self) {
        if let Some(installer) = &self.installer {
            let _ = installer.terminate(1);
            let _ = installer.process.wait(timing::RUNNER_KILL_WAIT);
        }
    }
    /// Closes the step-4 handle, then `ResumeThread`.
    fn resume_installer(&mut self) -> Result<(), String> {
        self.locked = None;
        self.installer
            .as_mut()
            .ok_or_else(|| "no installer process".to_string())?
            .resume()
            .map_err(|error| error.to_string())
    }
    fn wait_installer(&mut self, timeout: Duration) -> Option<u32> {
        let installer = self.installer.as_ref()?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match installer.process.wait(remaining) {
                Ok(code) => return code,
                Err(error) => {
                    self.log
                        .line(&format!("waiting for the installer: {error}"));
                    if remaining.is_zero() {
                        return None;
                    }
                    std::thread::sleep(remaining.min(Duration::from_secs(1)));
                }
            }
        }
    }
    fn relaunch_gui(&mut self) -> Result<(), String> {
        launch_via_shell(
            &self.installed("mklm.exe"),
            GUI_AFTER_UPDATE_ARG,
            &self.build.install_dir,
            timing::RELAUNCH_WAIT,
        )
        .map_err(|error| error.to_string())
    }
    fn log(&mut self, line: &str) {
        self.log.line(line);
    }
}
