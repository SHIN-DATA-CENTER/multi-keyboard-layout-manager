//! H2's driver against a fake machine (design m5b F.2 "H2 の駆動部"; OPS-UX-TEST-10): for the
//! rows of D.12, one failure injected at each step of D.7, and the final `Run`, `LastResult`,
//! relaunch and exit code; plus the orders the design fixes.
//!
//! The manifests are signed in the test with throwaway `minisign` keys (never stored).

use std::collections::{HashSet, VecDeque};
use std::io::Cursor;

use mklm_core::{Journal, Liveness, OpState};
use sha2::{Digest, Sha256};

use super::*;
use crate::keys::{KeyError, KeyId, KeyRole};
use crate::run::{InstallerExit, classify_run};
use crate::{DEFAULT_VALIDITY_DAYS, TRUSTED_COMMENT_PREFIX};

const RUN: &str = "0.2.1-3f9a0c2b7d1e4a65";
const BOOT: BootId = BootId(0x0b6d_3c2a_9e1f_4d5a_8c7b_6a5f_4e3d_2c1b);
const NOW_UNIX: u64 = 1_792_108_800;
const ISSUED_AT: u64 = 1_792_022_400;
const OLD_ID: &str = "0.2.0+0123456789abcdef";
const NEW_ID: &str = "0.2.1+fedcba9876543210";

const CALLER: u32 = 8532;
const STAGER: u32 = 9120;
const RUNNER: u32 = 9344;
const INSTALLER: u32 = 9512;
const OTHER_GUI: u32 = 7000;

fn process(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        creation_time: 134_041_234_000_000_000 + u64::from(pid),
    }
}

// ---- Signed manifests (throwaway keys) ----

struct Signer {
    pair: minisign::KeyPair,
    anchors: TrustAnchors,
}

/// A primary and a backup key.
fn signer() -> Signer {
    let primary = minisign::KeyPair::generate_unencrypted_keypair().expect("a throwaway key");
    let backup = minisign::KeyPair::generate_unencrypted_keypair().expect("a throwaway key");
    let anchors = TrustAnchors::from_keys(
        &[
            (KeyRole::Primary, &primary.pk.to_base64()),
            (KeyRole::Backup, &backup.pk.to_base64()),
        ],
        &[],
    )
    .unwrap_or_else(|error: KeyError| panic!("throwaway keys refused: {error}"));
    Signer {
        pair: primary,
        anchors,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn installer_bytes() -> Vec<u8> {
    (0..200_000u32).map(|i| (i * 31 % 251) as u8).collect()
}

impl Signer {
    fn key_id(&self) -> String {
        let keynum: [u8; 8] = self.pair.pk.keynum().try_into().expect("8 bytes");
        KeyId(keynum).to_text()
    }

    /// latest.json for 0.2.1 with `installer` as the x64 asset, and its signature.
    fn manifest(&self, version: &str, issued_at: u64, installer: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let expires = issued_at + DEFAULT_VALIDITY_DAYS * 86_400;
        let json = format!(
            "{{\n  \"schema\": 1,\n  \"product\": \"MKLM\",\n  \"channel\": \"stable\",\n  \
             \"version\": \"{version}\",\n  \"issued_at\": {issued_at},\n  \"expires\": {expires},\n  \
             \"key_ids\": [\"{key}\"],\n  \"revoked_keys\": [],\n  \"assets\": [\n    {{\n      \
             \"arch\": \"x64\",\n      \"name\": \"MKLM-Setup-{version}-x64.exe\",\n      \
             \"size\": {size},\n      \"sha256\": \"{sha}\"\n    }},\n    {{\n      \
             \"arch\": \"arm64\",\n      \"name\": \"MKLM-Setup-{version}-arm64.exe\",\n      \
             \"size\": 6029312,\n      \"sha256\": \"{arm}\"\n    }}\n  ]\n}}\n",
            key = self.key_id(),
            size = installer.len(),
            sha = hex(&Sha256::digest(installer)),
            arm = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        );
        let comment = format!("{TRUSTED_COMMENT_PREFIX} version={version} issued_at={issued_at}");
        let signature = minisign::sign(
            Some(&self.pair.pk),
            &self.pair.sk,
            Cursor::new(json.as_bytes()),
            Some(&comment),
            None,
        )
        .expect("sign")
        .into_string();
        (json.into_bytes(), signature.into_bytes())
    }
}

// ---- The fake machine ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallerProcess {
    NotCreated,
    Suspended,
    Running,
    Terminated,
    Exited,
}

struct Fake {
    version: Version,
    build_id: String,
    anchors: Option<TrustAnchors>,
    is_runner: Result<bool, String>,
    run: Option<RunRecord>,
    last_result: Option<UpdateResult>,
    trust: TrustState,
    manifest: Vec<u8>,
    signature: Vec<u8>,
    installer: Vec<u8>,
    lock_installer_fails: bool,
    installer_locked: bool,
    lock_result: Result<(), UpdateRefusal>,
    lock_held: bool,
    journal: Result<Journal, String>,
    installed_helper: Result<Option<String>, String>,
    free: Result<u64, String>,
    install_state: InstallState,
    /// What the installer leaves once it has run.
    after_install: InstallState,
    /// Processes that do not exit when waited for.
    stuck: HashSet<u32>,
    answers: Vec<(InstanceAnswer, u32)>,
    programs: Vec<RunningProgram>,
    files_in_use: (Vec<ProgramKind>, Vec<FileHolder>),
    session_ending: bool,
    claimed: bool,
    spawn: Result<ProcessIdentity, u32>,
    installer_process: InstallerProcess,
    /// Answers of successive `wait_installer` calls.
    installer_waits: VecDeque<Option<u32>>,
    fail_write_run: Option<RunPhase>,
    fail_last_result: bool,
    fail_delete_run: bool,
    resume_fails: bool,
    relaunch: Result<(), String>,
    /// Every `LastResult` written, in order.
    results: Vec<UpdateResult>,
    events: Vec<String>,
}

impl Fake {
    fn staged() -> Fake {
        let run = RunRecord {
            schema: 1,
            run_id: RunId::parse(RUN).unwrap(),
            from_version: "0.2.0".to_string(),
            to_version: "0.2.1".to_string(),
            arch: Arch::X64,
            phase: RunPhase::Staged,
            boot_id: BOOT,
            started_at: Timestamp(1_792_022_460_000),
            phase_at: Timestamp(1_792_022_470_000),
            caller: Some(process(CALLER)),
            caller_session: Some(1),
            stager: process(STAGER),
            runner: None,
            installer: None,
        };
        let old = InstallState::from_build_ids([
            Some(OLD_ID.to_string()),
            Some(OLD_ID.to_string()),
            Some(OLD_ID.to_string()),
        ]);
        let new = InstallState::from_build_ids([
            Some(NEW_ID.to_string()),
            Some(NEW_ID.to_string()),
            Some(NEW_ID.to_string()),
        ]);
        Fake {
            version: Version::new(0, 2, 0),
            build_id: OLD_ID.to_string(),
            anchors: None,
            is_runner: Ok(true),
            run: Some(run),
            last_result: None,
            trust: TrustState::default(),
            manifest: Vec::new(),
            signature: Vec::new(),
            installer: installer_bytes(),
            lock_installer_fails: false,
            installer_locked: false,
            lock_result: Ok(()),
            lock_held: false,
            journal: Ok(Journal::default()),
            installed_helper: Ok(Some(OLD_ID.to_string())),
            free: Ok(50 * 1024 * 1024 * 1024),
            install_state: old,
            after_install: new,
            stuck: HashSet::new(),
            answers: vec![(InstanceAnswer::Quit, 2), (InstanceAnswer::NotOurs, 3)],
            programs: vec![RunningProgram {
                identity: process(OTHER_GUI),
                kind: ProgramKind::Gui,
                session_id: 2,
            }],
            files_in_use: (Vec::new(), Vec::new()),
            session_ending: false,
            claimed: false,
            spawn: Ok(process(INSTALLER)),
            installer_process: InstallerProcess::NotCreated,
            installer_waits: VecDeque::from([Some(0)]),
            fail_write_run: None,
            fail_last_result: false,
            fail_delete_run: false,
            resume_fails: false,
            relaunch: Ok(()),
            results: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Staged, with a manifest signed for 0.2.1 and the matching installer.
    fn signed(signer: Signer) -> Fake {
        let mut fake = Fake::staged();
        let (manifest, signature) = signer.manifest("0.2.1", ISSUED_AT, &fake.installer);
        fake.manifest = manifest;
        fake.signature = signature;
        fake.anchors = Some(signer.anchors);
        fake
    }

    fn event(&mut self, event: impl Into<String>) {
        self.events.push(event.into());
    }

    fn position(&self, event: &str) -> usize {
        self.events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no {event} in {:?}", self.events))
    }

    fn has(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == event)
    }

    fn outcome(&self) -> UpdateOutcome {
        self.last_result
            .as_ref()
            .expect("a LastResult")
            .outcome
            .clone()
    }
}

impl RunnerEnv for Fake {
    fn version(&self) -> &Version {
        &self.version
    }
    fn arch(&self) -> Arch {
        Arch::X64
    }
    fn build_id(&self) -> &str {
        &self.build_id
    }
    fn anchors(&self) -> &TrustAnchors {
        self.anchors
            .as_ref()
            .expect("the driver verifies only after `ready`")
    }
    fn me(&self) -> ProcessIdentity {
        process(RUNNER)
    }
    fn boot_id(&self) -> BootId {
        BOOT
    }
    fn now(&self) -> Timestamp {
        Timestamp(NOW_UNIX * 1000)
    }
    fn now_unix(&self) -> u64 {
        NOW_UNIX
    }
    fn is_runner_of(&mut self, run_id: &RunId) -> Result<bool, String> {
        assert_eq!(run_id.as_str(), RUN);
        self.is_runner.clone()
    }
    fn read_run(&mut self) -> Result<Option<RunRecord>, String> {
        Ok(self.run.clone())
    }
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String> {
        if self.fail_write_run == Some(run.phase) {
            self.event(format!("write_run failed:{:?}", run.phase));
            return Err("RegSetValueExW failed with Win32 error 5".to_string());
        }
        self.event(format!("write_run:{:?}", run.phase));
        self.run = Some(run.clone());
        Ok(())
    }
    fn delete_run(&mut self) -> Result<(), String> {
        if self.fail_delete_run {
            return Err("RegDeleteValueW failed with Win32 error 5".to_string());
        }
        self.event("delete_run");
        self.run = None;
        Ok(())
    }
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String> {
        if self.fail_last_result {
            return Err("RegSetValueExW failed with Win32 error 5".to_string());
        }
        self.event("last_result");
        self.last_result = Some(result.clone());
        self.results.push(result.clone());
        Ok(())
    }
    fn read_trust(&mut self) -> TrustState {
        self.trust.clone()
    }
    fn lock_installer(&mut self, name: &str, max_len: u64) -> Result<(), String> {
        assert_eq!(name, "MKLM-Setup-0.2.1-x64.exe");
        assert_eq!(max_len, MAX_INSTALLER_LEN);
        if self.lock_installer_fails {
            return Err("CreateFileW failed with Win32 error 2".to_string());
        }
        self.event("lock_installer");
        self.installer_locked = true;
        Ok(())
    }
    fn read_staged_manifest(&mut self) -> Result<(Vec<u8>, Vec<u8>), String> {
        Ok((self.manifest.clone(), self.signature.clone()))
    }
    fn hash_locked_installer(&mut self) -> Result<(u64, Sha256Digest), String> {
        assert!(self.installer_locked, "hashed through the locked handle");
        let digest: [u8; 32] = Sha256::digest(&self.installer).into();
        Ok((self.installer.len() as u64, Sha256Digest(digest)))
    }
    fn cleanup_own_run_dir(&mut self) {
        self.event("cleanup");
        self.installer_locked = false;
    }
    fn sweep_other_run_dirs(&mut self) {
        self.event("sweep");
    }
    fn wait_for_exit(&mut self, process: &ProcessIdentity, timeout: Duration) -> bool {
        self.event(format!("wait_exit:{}:{}", process.pid, timeout.as_secs()));
        if self.stuck.contains(&process.pid) {
            return false;
        }
        self.programs
            .retain(|program| program.identity.pid != process.pid);
        true
    }
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal> {
        assert_eq!(timeout, timing::LOCK_WAIT_RUNNER);
        self.lock_result.clone()?;
        self.event("lock");
        self.lock_held = true;
        Ok(())
    }
    fn release_lock(&mut self) {
        assert!(self.lock_held, "released without holding it");
        self.event("unlock");
        self.lock_held = false;
    }
    fn read_journal(&mut self) -> Result<Journal, String> {
        assert!(self.lock_held, "the journal is read under the lock");
        self.journal.clone()
    }
    fn installed_helper_build_id(&mut self) -> Result<Option<String>, String> {
        self.installed_helper.clone()
    }
    fn free_space_program_files(&mut self) -> Result<u64, String> {
        self.free.clone()
    }
    fn read_install_state(&mut self) -> InstallState {
        self.install_state.clone()
    }
    fn quit_idle_instances(&mut self) -> Vec<(InstanceAnswer, u32)> {
        self.event("quit_idle");
        self.answers.clone()
    }
    fn running_programs(&mut self) -> Vec<RunningProgram> {
        self.programs.clone()
    }
    fn files_in_use(&mut self) -> (Vec<ProgramKind>, Vec<FileHolder>) {
        self.files_in_use.clone()
    }
    fn claim_installing(&mut self) -> bool {
        self.event("claim");
        if self.session_ending {
            return false;
        }
        self.claimed = true;
        true
    }
    fn set_block_reason(&mut self, on: bool) {
        self.event(if on { "block:on" } else { "block:off" });
    }
    fn release_installing(&mut self) {
        self.event("release_installing");
        self.claimed = false;
    }
    fn spawn_installer_suspended(&mut self) -> Result<ProcessIdentity, u32> {
        assert!(
            self.claimed,
            "the installer starts only after the session end was claimed"
        );
        let spawned = self.spawn;
        if spawned.is_ok() {
            self.event("spawn");
            self.installer_process = InstallerProcess::Suspended;
        }
        spawned
    }
    fn terminate_suspended_installer(&mut self) {
        assert_eq!(self.installer_process, InstallerProcess::Suspended);
        self.event("terminate");
        self.installer_process = InstallerProcess::Terminated;
    }
    fn resume_installer(&mut self) -> Result<(), String> {
        assert_eq!(self.installer_process, InstallerProcess::Suspended);
        // RELIABILITY-4: the record knows the installer before it runs.
        assert_eq!(
            self.run.as_ref().and_then(|run| run.installer),
            Some(process(INSTALLER)),
            "Run.installer is written before the installer is resumed"
        );
        assert_eq!(
            self.run.as_ref().map(|run| run.phase),
            Some(RunPhase::Installing)
        );
        if self.resume_fails {
            return Err("ResumeThread failed with Win32 error 5".to_string());
        }
        self.event("resume");
        self.installer_locked = false;
        self.installer_process = InstallerProcess::Running;
        Ok(())
    }
    fn wait_installer(&mut self, timeout: Duration) -> Option<u32> {
        assert_eq!(self.installer_process, InstallerProcess::Running);
        self.event(format!("wait_installer:{}", timeout.as_secs()));
        let answer = self.installer_waits.pop_front().expect("a scripted wait");
        if answer.is_some() {
            self.installer_process = InstallerProcess::Exited;
            self.install_state = self.after_install.clone();
        }
        answer
    }
    fn relaunch_gui(&mut self) -> Result<(), String> {
        self.event("relaunch");
        self.relaunch.clone()
    }
    fn log(&mut self, _line: &str) {}
}

fn run(fake: &mut Fake) -> u32 {
    run_update(fake, &RunId::parse(RUN).unwrap())
}

// ---- Before `ready`: nothing is recorded ----

#[test]
fn before_ready_nothing_is_recorded() {
    let staged = Fake::staged().run;
    type Inject = Box<dyn Fn(&mut Fake)>;
    let cases: Vec<(&str, Inject)> = vec![
        ("not the runner", Box::new(|f| f.is_runner = Ok(false))),
        (
            "unverifiable folder",
            Box::new(|f| f.is_runner = Err("Insecure".into())),
        ),
        ("no record", Box::new(|f| f.run = None)),
        (
            "another run",
            Box::new(|f| {
                if let Some(run) = f.run.as_mut() {
                    run.run_id = RunId::parse("0.2.1-0000000000000000").unwrap();
                }
            }),
        ),
        (
            "still staging",
            Box::new(|f| {
                if let Some(run) = f.run.as_mut() {
                    run.phase = RunPhase::Staging;
                }
            }),
        ),
        (
            "already taken",
            Box::new(|f| {
                if let Some(run) = f.run.as_mut() {
                    run.runner = Some(process(1234));
                }
            }),
        ),
        (
            "another build",
            Box::new(|f| f.version = Version::new(0, 1, 9)),
        ),
        (
            "installer missing",
            Box::new(|f| f.lock_installer_fails = true),
        ),
        (
            "Run = ready not written",
            Box::new(|f| f.fail_write_run = Some(RunPhase::Ready)),
        ),
    ];
    for (label, inject) in cases {
        let mut fake = Fake::staged();
        inject(&mut fake);
        let before = fake.run.clone();
        assert_eq!(run(&mut fake), EXIT_NOT_INSTALLED, "{label}");
        assert_eq!(fake.last_result, None, "{label}");
        assert_eq!(fake.run, before, "{label}");
        assert!(!fake.has("relaunch"), "{label}");
        assert!(!fake.has("lock"), "{label}");
        assert!(!fake.has("cleanup"), "{label}");
    }
    assert!(staged.is_some());
}

// ---- The happy path and its order ----

#[test]
fn installs_and_relaunches_last() {
    let signer = signer();
    let mut fake = Fake::signed(signer);
    assert_eq!(run(&mut fake), EXIT_INSTALLED);
    assert_eq!(fake.outcome(), UpdateOutcome::Installed);
    let result = fake.last_result.clone().unwrap();
    assert_eq!(result.installer_exit, Some(0));
    assert_eq!(result.installed_version.as_deref(), Some("0.2.1"));
    assert!(result.gui_relaunch_attempted);
    assert_eq!(result.run_id.as_str(), RUN);
    assert_eq!(fake.run, None);
    assert!(!fake.lock_held);
    // D.7: ready before anything else is written; the lock before the journal; waiting before the
    // other programs are asked; the installer recorded before it is resumed; LastResult, Run,
    // lock, clean-up, relaunch last.
    let order = [
        "lock_installer",
        "write_run:Ready",
        "lock",
        "write_run:Waiting",
        "quit_idle",
        "claim",
        "spawn",
        "write_run:Installing",
        "block:on",
        "resume",
        "wait_installer:900",
        "block:off",
        "write_run:Finishing",
        "release_installing",
        "last_result",
        "delete_run",
        "unlock",
        "cleanup",
        "sweep",
        "relaunch",
    ];
    let positions: Vec<usize> = order.iter().map(|event| fake.position(event)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{:?}",
        fake.events
    );
    assert_eq!(fake.events.last().map(String::as_str), Some("relaunch"));
    // H1 and the caller were waited for, with the design's limits.
    assert!(fake.has(&format!("wait_exit:{STAGER}:30")));
    assert!(fake.has(&format!("wait_exit:{CALLER}:30")));
    assert!(fake.has(&format!("wait_exit:{OTHER_GUI}:30")));
}

// ---- One failure at each step after `ready` (design m5b D.12) ----

/// Runs a signed fake with `inject`, expects exit 7, `outcome`, and the end state of a failure
/// before the installer ran: no Run, lock released, clean-up, relaunch last.
fn fails_with(label: &str, inject: impl FnOnce(&mut Fake), outcome: UpdateOutcome) -> Fake {
    let signer = signer();
    let mut fake = Fake::signed(signer);
    inject(&mut fake);
    assert_eq!(run(&mut fake), EXIT_NOT_INSTALLED, "{label}");
    assert_eq!(fake.outcome(), outcome, "{label}: {:?}", fake.events);
    assert_eq!(fake.run, None, "{label}");
    assert!(!fake.lock_held, "{label}");
    assert!(!fake.has("resume"), "{label}");
    let result = fake.last_result.clone().unwrap();
    assert_eq!(result.installer_exit, None, "{label}");
    assert_eq!(
        result.installed_version.as_deref(),
        Some("0.2.0"),
        "{label}"
    );
    assert!(fake.position("last_result") < fake.position("delete_run"));
    assert!(fake.position("delete_run") < fake.position("cleanup"));
    if outcome != UpdateOutcome::NotInstalled(NotInstalledReason::SessionEnding) {
        assert!(result.gui_relaunch_attempted, "{label}");
        assert_eq!(
            fake.events.last().map(String::as_str),
            Some("relaunch"),
            "{label}"
        );
    }
    fake
}

fn refused(refusal: UpdateRefusal) -> UpdateOutcome {
    UpdateOutcome::NotInstalled(NotInstalledReason::Refused(refusal))
}

#[test]
fn verification_again_before_the_lock() {
    let fake = fails_with(
        "tampered manifest",
        |f| {
            let at = f.manifest.len() / 2;
            f.manifest[at] ^= 0x01;
        },
        refused(UpdateRefusal::BadSignature),
    );
    assert!(!fake.has("lock"));
    fails_with(
        "another installer",
        |f| f.installer[1000] ^= 0x01,
        refused(UpdateRefusal::InstallerHashMismatch),
    );
    fails_with(
        "a shorter installer",
        |f| {
            f.installer.pop();
        },
        refused(UpdateRefusal::InstallerSizeMismatch {
            expected: 200_000,
            received: 199_999,
        }),
    );
    // The machine record moved past this manifest meanwhile (SECURITY-5).
    fails_with(
        "rollback",
        |f| {
            let key = f.manifest_signer_key_id();
            f.trust.max_issued_at.insert(key, ISSUED_AT + 1);
        },
        refused(UpdateRefusal::Rollback {
            issued_at: ISSUED_AT,
            seen: ISSUED_AT + 1,
        }),
    );
}

impl Fake {
    /// The key ID in `key_ids` of the staged manifest.
    fn manifest_signer_key_id(&self) -> String {
        let text = String::from_utf8(self.manifest.clone()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["key_ids"][0].as_str().unwrap().to_string()
    }
}

#[test]
fn the_lock_the_journal_and_the_installed_copy() {
    let fake = fails_with(
        "busy",
        |f| f.lock_result = Err(UpdateRefusal::Busy),
        refused(UpdateRefusal::Busy),
    );
    assert!(!fake.has("unlock"), "never held");
    let mut pending_reboot = Journal::default();
    let (ops, baselines) = mklm_core::fixtures::schema_1_journal();
    let parsed = Journal::parse(&ops, &baselines);
    pending_reboot.entries = parsed.entries;
    if let Some(entry) = pending_reboot.entries.last_mut() {
        entry.state = OpState::PendingReboot;
    }
    fails_with(
        "an operation waits for a restart",
        move |f| f.journal = Ok(pending_reboot),
        refused(UpdateRefusal::OperationOpen {
            waiting_for_reboot: true,
        }),
    );
    fails_with(
        "journal unreadable",
        |f| f.journal = Err("RegOpenKeyExW failed".to_string()),
        refused(UpdateRefusal::JournalUnreadable),
    );
    fails_with(
        "installed by hand meanwhile",
        |f| f.installed_helper = Ok(Some("0.2.2+abcdef".to_string())),
        UpdateOutcome::NotInstalled(NotInstalledReason::InstalledVersionChanged {
            found: Some("0.2.2+abcdef".to_string()),
        }),
    );
    fails_with(
        "helper removed",
        |f| f.installed_helper = Ok(None),
        UpdateOutcome::NotInstalled(NotInstalledReason::InstalledVersionChanged { found: None }),
    );
    fails_with(
        "disk full",
        |f| f.free = Ok(1024),
        UpdateOutcome::NotInstalled(NotInstalledReason::DiskFull {
            needed: 200_000 * 4 + 64 * 1024 * 1024,
            available: 1024,
        }),
    );
    fails_with(
        "Run = waiting not written",
        |f| f.fail_write_run = Some(RunPhase::Waiting),
        refused(UpdateRefusal::Storage {
            detail: "RegSetValueExW failed with Win32 error 5".to_string(),
        }),
    );
}

#[test]
fn other_mklm_programs() {
    let fake = fails_with(
        "the caller did not exit",
        |f| {
            f.stuck.insert(CALLER);
        },
        UpdateOutcome::NotInstalled(NotInstalledReason::CallerDidNotExit),
    );
    assert!(!fake.has("quit_idle"));
    fails_with(
        "another user's MKLM is busy",
        |f| {
            f.answers = vec![
                (InstanceAnswer::Quit, 2),
                (InstanceAnswer::Busy, 3),
                (InstanceAnswer::NoAnswer, 4),
                (InstanceAnswer::Busy, 3),
                (InstanceAnswer::Busy, 5),
            ];
        },
        UpdateOutcome::NotInstalled(NotInstalledReason::InstanceBusy {
            sessions: vec![3, 5],
        }),
    );
    fails_with(
        "a CLI waits for input",
        |f| {
            f.programs.push(RunningProgram {
                identity: process(7120),
                kind: ProgramKind::Cli,
                session_id: 2,
            });
            f.stuck.insert(7120);
        },
        UpdateOutcome::NotInstalled(NotInstalledReason::ProgramsStillRunning {
            programs: vec![ProgramKind::Cli],
            holders: vec![FileHolder {
                pid: 7120,
                session_id: 2,
                name: "mklm-cli.exe".to_string(),
            }],
        }),
    );
    // A helper gets 75 s.
    let fake = fails_with(
        "a helper does not end",
        |f| {
            f.programs = vec![RunningProgram {
                identity: process(6000),
                kind: ProgramKind::Helper,
                session_id: 0,
            }];
            f.stuck.insert(6000);
        },
        UpdateOutcome::NotInstalled(NotInstalledReason::ProgramsStillRunning {
            programs: vec![ProgramKind::Helper],
            holders: vec![FileHolder {
                pid: 6000,
                session_id: 0,
                name: "mklm-helper.exe".to_string(),
            }],
        }),
    );
    assert!(fake.has("wait_exit:6000:75"));
    fails_with(
        "a file held without delete sharing",
        |f| {
            f.files_in_use = (
                vec![ProgramKind::Gui],
                vec![FileHolder {
                    pid: 7120,
                    session_id: 2,
                    name: "powershell.exe".to_string(),
                }],
            );
        },
        UpdateOutcome::NotInstalled(NotInstalledReason::FilesInUse {
            programs: vec![ProgramKind::Gui],
            holders: vec![FileHolder {
                pid: 7120,
                session_id: 2,
                name: "powershell.exe".to_string(),
            }],
        }),
    );
}

#[test]
fn the_session_end_before_the_installer() {
    let fake = fails_with(
        "sign-out during waiting",
        |f| f.session_ending = true,
        UpdateOutcome::NotInstalled(NotInstalledReason::SessionEnding),
    );
    assert!(!fake.has("spawn"), "no installer once the session ends");
    assert!(
        !fake.has("relaunch"),
        "the GUI would not outlive the sign-out"
    );
    assert!(!fake.last_result.clone().unwrap().gui_relaunch_attempted);
}

#[test]
fn the_installer_does_not_start() {
    // 225: blocked by antivirus software (design m5b D.7 step 16).
    let fake = fails_with(
        "blocked",
        |f| f.spawn = Err(225),
        UpdateOutcome::NotInstalled(NotInstalledReason::InstallerNotStarted { code: 225 }),
    );
    assert!(fake.position("claim") < fake.position("release_installing"));
    // Run = installing cannot be written: the never-run installer is terminated.
    let fake = fails_with(
        "Run = installing not written",
        |f| f.fail_write_run = Some(RunPhase::Installing),
        refused(UpdateRefusal::Storage {
            detail: "RegSetValueExW failed with Win32 error 5".to_string(),
        }),
    );
    assert!(fake.position("spawn") < fake.position("terminate"));
    assert!(!fake.has("block:on"));
    let fake = fails_with(
        "resume fails",
        |f| f.resume_fails = true,
        refused(UpdateRefusal::Internal {
            detail: "the installer could not be resumed: ResumeThread failed with Win32 error 5"
                .to_string(),
        }),
    );
    assert!(fake.position("block:on") < fake.position("terminate"));
    assert!(fake.position("terminate") < fake.position("block:off"));
}

// ---- After the installer (design m5b D.13) ----

fn after_installer(label: &str, exit: u32, after: InstallState, outcome: UpdateOutcome, code: u32) {
    let signer = signer();
    let mut fake = Fake::signed(signer);
    fake.installer_waits = VecDeque::from([Some(exit)]);
    fake.after_install = after;
    assert_eq!(run(&mut fake), code, "{label}");
    assert_eq!(fake.outcome(), outcome, "{label}");
    assert_eq!(fake.last_result.clone().unwrap().installer_exit, Some(exit));
    assert_eq!(fake.run, None);
    assert!(!fake.lock_held);
    assert_eq!(fake.events.last().map(String::as_str), Some("relaunch"));
}

fn state(ids: [Option<&str>; 3]) -> InstallState {
    InstallState::from_build_ids(ids.map(|id| id.map(str::to_string)))
}

#[test]
fn the_installers_result() {
    let old = state([Some(OLD_ID); 3]);
    let new = state([Some(NEW_ID); 3]);
    after_installer(
        "installed with a non-zero code",
        2,
        new,
        UpdateOutcome::Installed,
        EXIT_INSTALLED,
    );
    after_installer(
        "files in use (26)",
        26,
        old.clone(),
        UpdateOutcome::NotInstalled(NotInstalledReason::InstallerRefused {
            exit: InstallerExit::FilesInUse,
        }),
        EXIT_NOT_INSTALLED,
    );
    after_installer(
        "write failed (27)",
        27,
        old.clone(),
        UpdateOutcome::NotInstalled(NotInstalledReason::InstallerRefused {
            exit: InstallerExit::FileWrite,
        }),
        EXIT_NOT_INSTALLED,
    );
    after_installer(
        "script abort",
        2,
        old,
        UpdateOutcome::NotInstalled(NotInstalledReason::InstallerExit { code: 2 }),
        EXIT_NOT_INSTALLED,
    );
    after_installer(
        "half replaced",
        0,
        state([Some(NEW_ID), Some(OLD_ID), Some(NEW_ID)]),
        UpdateOutcome::Failed(FailedReason::Inconsistent),
        EXIT_NOT_INSTALLED,
    );
}

#[test]
fn a_slow_installer_is_never_stopped() {
    // Ends after 15 minutes but within 60: the timed-out result is replaced by the real one.
    let signer = signer();
    let mut fake = Fake::signed(signer);
    fake.installer_waits = VecDeque::from([None, Some(0)]);
    assert_eq!(run(&mut fake), EXIT_INSTALLED);
    assert_eq!(fake.results.len(), 2);
    assert_eq!(
        fake.results[0].outcome,
        UpdateOutcome::Failed(FailedReason::InstallerTimedOut)
    );
    assert!(!fake.results[0].gui_relaunch_attempted);
    assert_eq!(fake.results[1].outcome, UpdateOutcome::Installed);
    assert!(fake.has("wait_installer:900"));
    assert!(fake.has("wait_installer:2700"));
    assert!(!fake.has("terminate"));
    assert_eq!(fake.run, None);

    // Still running after 60 minutes: Run stays with the installer, nothing relaunched, 7.
    let second = self::signer();
    let mut fake = Fake::signed(second);
    fake.installer_waits = VecDeque::from([None, None]);
    assert_eq!(run(&mut fake), EXIT_NOT_INSTALLED);
    assert_eq!(
        fake.outcome(),
        UpdateOutcome::Failed(FailedReason::InstallerTimedOut)
    );
    let run_left = fake.run.clone().expect("Run stays");
    assert_eq!(run_left.phase, RunPhase::Installing);
    assert_eq!(run_left.installer, Some(process(INSTALLER)));
    assert!(!fake.has("terminate"));
    assert!(!fake.has("relaunch"));
    assert!(!fake.has("delete_run"));
    assert!(!fake.lock_held, "the lock is released");
    assert!(fake.has("block:off"));
    // While the installer lives, everyone sees the update in progress; once it is gone, the
    // record is interrupted and the files decide (D.13).
    let alive = |p: &ProcessIdentity| {
        if p.pid == INSTALLER {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    };
    assert!(matches!(
        classify_run(Some(&run_left), BOOT, &alive),
        crate::run::RunView::InProgress { .. }
    ));
    assert!(matches!(
        classify_run(Some(&run_left), BOOT, &|_| Liveness::Dead),
        crate::run::RunView::Interrupted(_)
    ));
}

#[test]
fn a_last_result_that_cannot_be_written_keeps_the_run() {
    let signer = signer();
    let mut fake = Fake::signed(signer);
    fake.fail_last_result = true;
    assert_eq!(run(&mut fake), EXIT_INSTALLED);
    assert_eq!(fake.last_result, None);
    let left = fake.run.clone().expect("Run stays for the next start");
    assert_eq!(left.phase, RunPhase::Finishing);
    assert!(!fake.lock_held);
    // A GUI started now would see this live runner in `Run` and quit at once (D.13 step 1):
    // no relaunch; the RunOnce value, the Run key or the user start it later (MECHANICS-2).
    assert!(!fake.has("relaunch"));
    assert!(fake.has("cleanup"), "the clean-up still runs");
}

/// `Run` that cannot be deleted after `LastResult` was written: no relaunch into a GUI that would
/// quit at once, and `LastResult` says so (D.7 steps 19 and 21; MECHANICS-2).
#[test]
fn a_run_that_cannot_be_deleted_gets_no_relaunch() {
    // Installed.
    let mut fake = Fake::signed(signer());
    fake.fail_delete_run = true;
    assert_eq!(run(&mut fake), EXIT_INSTALLED);
    let left = fake.run.clone().expect("Run stays");
    assert_eq!(left.phase, RunPhase::Finishing);
    assert!(!fake.has("relaunch"));
    assert!(!fake.lock_held);
    assert_eq!(fake.results.len(), 2, "written, then corrected");
    assert!(fake.results[0].gui_relaunch_attempted);
    let last = fake.last_result.clone().unwrap();
    assert_eq!(last.outcome, UpdateOutcome::Installed);
    assert!(!last.gui_relaunch_attempted);
    // The start gate of a GUI started while this runner lives: in progress, so it would quit.
    let runner = left.runner.expect("the runner is recorded");
    let alive = move |p: &ProcessIdentity| {
        if *p == runner {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    };
    assert!(matches!(
        classify_run(Some(&left), BOOT, &alive),
        crate::run::RunView::InProgress { .. }
    ));
    // Once it is gone, the next start reports the run as interrupted.
    assert!(matches!(
        classify_run(Some(&left), BOOT, &|_| Liveness::Dead),
        crate::run::RunView::Interrupted(_)
    ));

    // Not installed (a refusal after `ready`): the same.
    let mut fake = Fake::signed(signer());
    fake.fail_delete_run = true;
    fake.lock_result = Err(UpdateRefusal::Busy);
    assert_eq!(run(&mut fake), EXIT_NOT_INSTALLED);
    assert!(matches!(fake.outcome(), UpdateOutcome::NotInstalled(_)));
    assert!(fake.run.is_some());
    assert!(!fake.has("relaunch"));
    assert!(!fake.last_result.clone().unwrap().gui_relaunch_attempted);

    // Deleted: relaunched, and recorded as attempted (the control).
    let mut fake = Fake::signed(signer());
    assert_eq!(run(&mut fake), EXIT_INSTALLED);
    assert_eq!(fake.run, None);
    assert!(fake.has("relaunch"));
    assert_eq!(fake.results.len(), 1);
    assert!(fake.results[0].gui_relaunch_attempted);
}

#[test]
fn a_failed_relaunch_changes_nothing() {
    let signer = signer();
    let mut fake = Fake::signed(signer);
    fake.relaunch = Err("ShellWindows: access denied".to_string());
    assert_eq!(run(&mut fake), EXIT_INSTALLED);
    assert_eq!(fake.outcome(), UpdateOutcome::Installed);
    assert!(fake.last_result.clone().unwrap().gui_relaunch_attempted);
}

/// Design m5b D.7 "サインアウトとシャットダウン" (RELIABILITY-3).
#[test]
fn the_session_end_gate() {
    // Before the installer: the end is let through, and the installer then never starts.
    let gate = SessionEndGate::new();
    assert!(!gate.session_ending());
    assert!(!gate.query_end_session(), "not blocked while waiting");
    assert!(gate.session_ending());
    assert!(!gate.claim_installing());
    assert!(!gate.query_end_session());
    // Another application cancelled the end: the update may go on.
    let gate = SessionEndGate::new();
    assert!(!gate.query_end_session());
    gate.end_session(false);
    assert!(!gate.session_ending());
    assert!(gate.claim_installing());
    // While installing: blocked, whatever else happens.
    assert!(gate.query_end_session());
    gate.end_session(false);
    assert!(gate.query_end_session());
    gate.end_session(true);
    assert!(gate.query_end_session());
    // Finishing: let through again, and no second installer.
    gate.release_installing();
    assert!(!gate.query_end_session());
    assert!(!gate.claim_installing());
    // Released without an installer (it could not be started).
    let gate = SessionEndGate::new();
    assert!(gate.claim_installing());
    gate.release_installing();
    assert!(!gate.query_end_session());
}

#[test]
fn file_names_of_the_programs() {
    assert_eq!(program_file_name(ProgramKind::Gui), "mklm.exe");
    assert_eq!(program_file_name(ProgramKind::Cli), "mklm-cli.exe");
    assert_eq!(program_file_name(ProgramKind::Helper), "mklm-helper.exe");
}
