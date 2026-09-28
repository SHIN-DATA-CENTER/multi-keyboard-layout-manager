//! The update session end to end in memory (design m5b F.4): the caller's `update::stage` against
//! the helper's `mklm_ipc::staging::stage_update` (H1) over a duplex link of channels, with a fake
//! `StagerEnv` in place of the machine. Nothing is written outside memory and a scratch folder.

mod update_common;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mklm_client::session::{Link, Recv};
use mklm_client::update::check::{CheckOutcome, Offer, check};
use mklm_client::update::download::download;
use mklm_client::update::stage::{HandOffProbe, StageEnd, StageFrontend, stage};
use mklm_core::{BootId, Journal, Liveness, ProcessIdentity, Timestamp};
use mklm_ipc::staging::{StageLink, StageSink, StagedInstaller, StagerEnv, stage_update};
use mklm_ipc::{CallerMessage, FrameError, HelperMessage};
use mklm_update::run::{InstallState, RunId, RunPhase, RunRecord, UpdateResult};
use mklm_update::{Arch, TrustAnchors, TrustState, UpdateRefusal, Version};
use update_common::*;

/// The caller's end of the pipe.
struct CallerEnd {
    to_helper: Sender<CallerMessage>,
    from_helper: Receiver<HelperMessage>,
}

impl Link for CallerEnd {
    fn send(&mut self, message: &CallerMessage) -> Result<(), String> {
        self.to_helper
            .send(message.clone())
            .map_err(|_| "the helper closed the connection".to_string())
    }

    fn recv(&mut self, timeout: Duration) -> Recv {
        match self.from_helper.recv_timeout(timeout) {
            Ok(message) => Recv::Message(message),
            Err(RecvTimeoutError::Timeout) => Recv::Timeout,
            Err(RecvTimeoutError::Disconnected) => {
                Recv::Closed("the helper closed the connection".into())
            }
        }
    }

    fn helper_exit_code(&mut self) -> Option<u32> {
        None
    }
}

/// The helper's end of the pipe.
struct HelperEnd {
    to_caller: Sender<HelperMessage>,
    from_caller: Receiver<CallerMessage>,
}

impl StageLink for HelperEnd {
    fn send(&mut self, message: HelperMessage) -> Result<(), String> {
        self.to_caller
            .send(message)
            .map_err(|_| "the caller left".to_string())
    }

    fn recv(&mut self, timeout: Duration) -> Result<CallerMessage, FrameError> {
        match self.from_caller.recv_timeout(timeout) {
            Ok(message) => Ok(message),
            Err(RecvTimeoutError::Timeout) => Err(FrameError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(FrameError::Closed),
        }
    }
}

fn pipe() -> (CallerEnd, HelperEnd) {
    let (to_helper, from_caller) = mpsc::channel();
    let (to_caller, from_helper) = mpsc::channel();
    (
        CallerEnd {
            to_helper,
            from_helper,
        },
        HelperEnd {
            to_caller,
            from_caller,
        },
    )
}

/// What the fake machine keeps.
#[derive(Debug, Default)]
struct Machine {
    trust: TrustState,
    run: Option<RunRecord>,
    last_result: Option<UpdateResult>,
    files: Vec<(String, Vec<u8>)>,
    installer: Arc<Mutex<Vec<u8>>>,
    committed: bool,
    runner: Option<ProcessIdentity>,
    removed: Vec<String>,
    log: Vec<String>,
}

/// H1's machine: in memory; the runner becomes ready as soon as it is started.
struct FakeMachine {
    version: Version,
    anchors: TrustAnchors,
    free: u64,
    state: Arc<Mutex<Machine>>,
}

struct MemoryInstaller {
    bytes: Arc<Mutex<Vec<u8>>>,
    state: Arc<Mutex<Machine>>,
}

impl StageSink for MemoryInstaller {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(())
    }
}

impl StagedInstaller for MemoryInstaller {
    fn commit(self: Box<Self>) -> Result<(), String> {
        self.state.lock().unwrap().committed = true;
        Ok(())
    }
}

fn identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        creation_time: 134_041_234_567_890_123 + u64::from(pid),
    }
}

impl StagerEnv for FakeMachine {
    fn version(&self) -> &Version {
        &self.version
    }
    fn arch(&self) -> Arch {
        Arch::of_this_build()
    }
    fn anchors(&self) -> &TrustAnchors {
        &self.anchors
    }
    fn me(&self) -> ProcessIdentity {
        identity(9120)
    }
    fn boot_id(&self) -> BootId {
        BootId(0x0b6d_3c2a_9e1f_4d5a_8c7b_6a5f_4e3d_2c1b)
    }
    fn now(&self) -> Timestamp {
        Timestamp(NOW * 1000)
    }
    fn now_unix(&self) -> u64 {
        NOW
    }
    fn runs_from_install_dir(&mut self) -> bool {
        true
    }
    fn caller(&mut self) -> (Option<ProcessIdentity>, Option<u32>) {
        (Some(identity(8532)), Some(1))
    }
    fn free_space_program_data(&mut self) -> Result<u64, String> {
        Ok(self.free)
    }
    fn acquire_lock(&mut self, _timeout: Duration) -> Result<(), UpdateRefusal> {
        Ok(())
    }
    fn release_lock(&mut self) {}
    fn read_journal(&mut self) -> Result<Journal, String> {
        Ok(Journal::default())
    }
    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        if self.state.lock().unwrap().runner.as_ref() == Some(process) {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    }
    fn read_install_state(&mut self) -> InstallState {
        InstallState::default()
    }
    fn read_trust(&mut self) -> TrustState {
        self.state.lock().unwrap().trust.clone()
    }
    fn write_trust(&mut self, trust: &TrustState) -> Result<(), String> {
        self.state.lock().unwrap().trust = trust.clone();
        Ok(())
    }
    fn read_run(&mut self) -> Result<Option<RunRecord>, String> {
        let mut state = self.state.lock().unwrap();
        // The runner (H2) got ready as soon as it was started.
        let runner = state.runner;
        if let (Some(run), Some(runner)) = (state.run.as_mut(), runner)
            && run.phase == RunPhase::Staged
        {
            run.phase = RunPhase::Ready;
            run.runner = Some(runner);
        }
        Ok(state.run.clone())
    }
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String> {
        self.state.lock().unwrap().run = Some(run.clone());
        Ok(())
    }
    fn delete_run(&mut self) -> Result<(), String> {
        self.state.lock().unwrap().run = None;
        Ok(())
    }
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String> {
        self.state.lock().unwrap().last_result = Some(result.clone());
        Ok(())
    }
    fn sweep_run_dirs(&mut self, _keep: Option<&RunId>) {}
    fn random_suffix(&mut self) -> [u8; 8] {
        [0x3f, 0x9a, 0x0c, 0x2b, 0x7d, 0x1e, 0x4a, 0x65]
    }
    fn create_run_dir(&mut self, _run_id: &RunId) -> Result<(), String> {
        Ok(())
    }
    fn remove_run_dir(&mut self, run_id: &RunId) {
        self.state
            .lock()
            .unwrap()
            .removed
            .push(run_id.as_str().to_string());
    }
    fn write_run_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.state
            .lock()
            .unwrap()
            .files
            .push((name.to_string(), bytes.to_vec()));
        Ok(())
    }
    fn create_installer(&mut self, _name: &str) -> Result<Box<dyn StagedInstaller>, String> {
        let bytes = self.state.lock().unwrap().installer.clone();
        Ok(Box::new(MemoryInstaller {
            bytes,
            state: self.state.clone(),
        }))
    }
    fn copy_self_as_runner(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn create_tmp_dir(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn spawn_runner(&mut self, _run_id: &RunId) -> Result<ProcessIdentity, String> {
        let runner = identity(9344);
        self.state.lock().unwrap().runner = Some(runner);
        Ok(runner)
    }
    fn runner_exit_code(&mut self) -> Option<u32> {
        None
    }
    fn stop_runner(&mut self, _wait: Duration) {}
    fn sleep(&mut self, duration: Duration) {
        thread::sleep(duration.min(Duration::from_millis(5)));
    }
    fn log(&mut self, line: &str) {
        self.state.lock().unwrap().log.push(line.to_string());
    }
}

#[derive(Debug, Default)]
struct Frontend {
    sent: u64,
    starting: bool,
}

impl StageFrontend for Frontend {
    fn sent(&mut self, bytes: u64, _total: u64) {
        self.sent = bytes;
    }
    fn received(&mut self, _bytes: u64) {}
    fn starting_runner(&mut self) {
        self.starting = true;
    }
    fn cancel_requested(&mut self) -> bool {
        false
    }
}

struct NoProbe;

impl HandOffProbe for NoProbe {
    fn handed_off(&mut self) -> Option<(String, String)> {
        None
    }
}

/// A checked and downloaded offer of 0.2.1 in a scratch cache.
fn downloaded_offer(scratch: &Scratch) -> (Offer, std::path::PathBuf) {
    let cache = mklm_client::update::cache::UpdateCache::new(scratch.0.clone());
    let env = env("0.2.0", scratch.0.clone());
    let mut github = FakeGitHub::new(new_release());
    let outcome = check(
        &mut github,
        &env,
        &cache,
        &TrustState::default(),
        None,
        NOW,
        &Default::default(),
    )
    .unwrap();
    let CheckOutcome::Available(offer) = outcome else {
        panic!("{outcome:?}");
    };
    let path = download(
        &mut github,
        &env,
        &cache,
        &offer,
        &mut |_, _| {},
        &Default::default(),
    )
    .unwrap();
    (offer, path)
}

/// Runs H1 on its own thread for one `StageUpdate`, then until the caller leaves.
fn serve(machine: FakeMachine, mut end: HelperEnd) -> thread::JoinHandle<Arc<Mutex<Machine>>> {
    thread::spawn(move || {
        let state = machine.state.clone();
        let mut machine = machine;
        while let Ok(message) = end.recv(Duration::from_secs(20)) {
            match message {
                CallerMessage::StageUpdate(request) => {
                    stage_update(&mut machine, &mut end, &request);
                }
                CallerMessage::Bye => break,
                _ => {}
            }
        }
        state
    })
}

fn machine(free: u64) -> FakeMachine {
    FakeMachine {
        version: Version::new(0, 2, 0),
        anchors: anchors(),
        free,
        state: Arc::default(),
    }
}

#[test]
fn the_installer_reaches_h1_and_the_runner_takes_over() {
    let scratch = Scratch::new("staging-ok");
    let (offer, path) = downloaded_offer(&scratch);
    let (mut caller, helper) = pipe();
    let h1 = serve(machine(u64::MAX / 2), helper);
    let mut frontend = Frontend::default();
    let end = stage(&mut caller, &offer, &path, &mut frontend, &mut NoProbe);
    drop(caller);
    let state = h1.join().unwrap();
    assert_eq!(
        end,
        StageEnd::HandedOff {
            run_id: "0.2.1-3f9a0c2b7d1e4a65".into(),
            to_version: "0.2.1".into()
        }
    );
    assert!(frontend.starting);
    assert_eq!(frontend.sent, offer.verified.asset.size);
    let state = state.lock().unwrap();
    assert!(state.committed);
    assert_eq!(
        *state.installer.lock().unwrap(),
        std::fs::read(&path).unwrap()
    );
    // The manifest and the signature that verified, as received (design m5b D.4 step 12).
    assert!(
        state
            .files
            .iter()
            .any(|(name, bytes)| name == "latest.json" && *bytes == offer.manifest)
    );
    assert!(
        state
            .files
            .iter()
            .any(|(name, bytes)| name == "latest.json.minisig" && *bytes == offer.signature)
    );
    // The machine record learned the manifest (D.4 step 9).
    assert!(state.trust.max_issued_at.contains_key(PRIMARY_ID));
}

#[test]
fn a_refusal_changes_nothing() {
    let scratch = Scratch::new("staging-refused");
    let (offer, path) = downloaded_offer(&scratch);
    let (mut caller, helper) = pipe();
    // Not enough space on the volume of %ProgramData% (D.4 step 4).
    let h1 = serve(machine(1024), helper);
    let end = stage(
        &mut caller,
        &offer,
        &path,
        &mut Frontend::default(),
        &mut NoProbe,
    );
    drop(caller);
    let state = h1.join().unwrap();
    assert!(
        matches!(end, StageEnd::Refused(UpdateRefusal::DiskFull { .. })),
        "{end:?}"
    );
    let state = state.lock().unwrap();
    assert!(state.installer.lock().unwrap().is_empty());
    assert_eq!(state.run, None);
}
