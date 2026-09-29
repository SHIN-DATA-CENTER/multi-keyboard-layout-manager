//! The helper's side of an update session (design m5b F.2): `receive_installer` against a
//! scripted caller, H1's driver `stage_update` and the `RecordTrust` handling against a fake
//! machine (a failure injected at each step of D.4), and the order rule `CallerOrder`.
//!
//! The manifests below were signed once with a throwaway minisign key pair whose secret key was
//! discarded (mklm-ipc has no minisign dependency to make one here). Only the public keys are
//! kept; they are trusted by nothing but these tests (`TrustAnchors::from_keys`).

use std::collections::VecDeque;
use std::time::Duration;

use mklm_core::{BootId, Journal, JournalEntry, Liveness, OpState, ProcessIdentity, Timestamp};
use mklm_ipc::staging::{
    CallerOrder, PROGRESS_EVERY, StageFlowEnd, StageLink, StageSink, StagedInstaller, StagerEnv,
    StagingEnd, receive_installer, record_trust, stage_update,
};
use mklm_ipc::{
    CHUNK_LEN, CallerMessage, FrameError, HelperMessage, InstallerChunk, Request,
    StageUpdateRequest, TrustReport, UpdateMessage, UpdateRefusal, Welcome, decode_hex,
};
use mklm_update::run::{
    InstallState, RunId, RunPhase, RunRecord, UpdateOutcome, UpdateResult, timing,
};
use mklm_update::stage::{StagePlan, Stager};
use mklm_update::{
    Arch, KeyError, KeyRole, SelectedAsset, Sha256Digest, TrustAnchors, TrustState, Version,
};

// ---- Fixtures (throwaway key, secret discarded) ----

const PRIMARY_KEY: &str = "RWRbYPTZMfF0498oIbRKCnDiaiFRKcQXD9JESIpOCKQ/5eLA9q/VFLXn";
const BACKUP_KEY: &str = "RWTkqIo0o7NEpx7P2akhtN0bFMLr5EIS9/PgbUICPNbRKcY5znfH65Ww";
const KEY_ID: &str = "E374F131D9F4605B";
const INSTALLER_SIZE: usize = 1_200_000;
const INSTALLER_SHA256: &str = "5827eb08a67031bdd98166818c3c5719a1c6158db6ed39960498aa7a73dbda13";
const OLD_ISSUED_AT: u64 = 1_792_022_400;
const NEW_ISSUED_AT: u64 = 1_792_108_800;
const NOW_UNIX: u64 = 1_792_195_200;
const _: () = assert!(OLD_ISSUED_AT < NEW_ISSUED_AT && NEW_ISSUED_AT < NOW_UNIX);

/// 0.2.1, issued 2026-10-15.
const MANIFEST_OLD: &str = "{\n  \"schema\": 1,\n  \"product\": \"MKLM\",\n  \"channel\": \"stable\",\n  \"version\": \"0.2.1\",\n  \"issued_at\": 1792022400,\n  \"expires\": 1807574400,\n  \"key_ids\": [\n    \"E374F131D9F4605B\"\n  ],\n  \"revoked_keys\": [],\n  \"assets\": [\n    {\n      \"arch\": \"x64\",\n      \"name\": \"MKLM-Setup-0.2.1-x64.exe\",\n      \"size\": 1200000,\n      \"sha256\": \"5827eb08a67031bdd98166818c3c5719a1c6158db6ed39960498aa7a73dbda13\"\n    },\n    {\n      \"arch\": \"arm64\",\n      \"name\": \"MKLM-Setup-0.2.1-arm64.exe\",\n      \"size\": 6029312,\n      \"sha256\": \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n    }\n  ]\n}\n";
const SIGNATURE_OLD: &str = "untrusted comment: signature from rsign secret key\nRURbYPTZMfF04wDwlv7gmMj7E5NtsmRJfydiKKNPzGnidoNC2hvuLQEpPRHQ7nKMnFe1PMey3y0VF9cCkmGbn0lH+l55/+0k5Ac=\ntrusted comment: mklm-latest-json v1 version=0.2.1 issued_at=1792022400\n0fWXmBg4w43VfuNLfJMddvSX6Mhu7BHfndip1IZqRX0B3CXMrZx5vxDyXuF8FkKhwRtg620P1UABHEMrPz9xCg==\n";
/// The same release, signed again a day later.
const MANIFEST_NEW: &str = "{\n  \"schema\": 1,\n  \"product\": \"MKLM\",\n  \"channel\": \"stable\",\n  \"version\": \"0.2.1\",\n  \"issued_at\": 1792108800,\n  \"expires\": 1807660800,\n  \"key_ids\": [\n    \"E374F131D9F4605B\"\n  ],\n  \"revoked_keys\": [],\n  \"assets\": [\n    {\n      \"arch\": \"x64\",\n      \"name\": \"MKLM-Setup-0.2.1-x64.exe\",\n      \"size\": 1200000,\n      \"sha256\": \"5827eb08a67031bdd98166818c3c5719a1c6158db6ed39960498aa7a73dbda13\"\n    },\n    {\n      \"arch\": \"arm64\",\n      \"name\": \"MKLM-Setup-0.2.1-arm64.exe\",\n      \"size\": 6029312,\n      \"sha256\": \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n    }\n  ]\n}\n";
const SIGNATURE_NEW: &str = "untrusted comment: signature from rsign secret key\nRURbYPTZMfF040YY4iTTyhKyZz3Hk3zFOqurpCoO/dnBXccaSteWKXW4gCrxjSNKgcG84lqS6H67wBz8JGSuIT4EW33mGxKctAY=\ntrusted comment: mklm-latest-json v1 version=0.2.1 issued_at=1792108800\nOiasXDJmA5pUzVVfokvXKSnPWhJWavfkHTpRf67hZbLXOqLJeVhzu71zkDGcN/iVOnRRlWRMZj00g9p8uk7fAA==\n";

/// The installer both manifests describe.
fn installer() -> Vec<u8> {
    (0..INSTALLER_SIZE as u32)
        .map(|i| ((i * 31 + 7) % 251) as u8)
        .collect()
}

fn digest(hex: &str) -> Sha256Digest {
    let bytes = decode_hex(hex).expect("64 hex digits");
    Sha256Digest(bytes.try_into().expect("32 bytes"))
}

/// The fixture keys.
fn anchors() -> TrustAnchors {
    TrustAnchors::from_keys(
        &[
            (KeyRole::Primary, PRIMARY_KEY),
            (KeyRole::Backup, BACKUP_KEY),
        ],
        &[],
    )
    .unwrap_or_else(|error: KeyError| panic!("the fixture keys are refused: {error}"))
}

const RUN: &str = "0.2.1-0102030405060708";

fn plan(size: u64, sha256: Sha256Digest) -> StagePlan {
    StagePlan {
        run_id: RunId::parse(RUN).unwrap(),
        from_version: Version::new(0, 2, 0),
        to_version: Version::new(0, 2, 1),
        asset: SelectedAsset {
            arch: Arch::X64,
            name: "MKLM-Setup-0.2.1-x64.exe".to_string(),
            size,
            sha256,
        },
    }
}

// ---- A scripted caller ----

/// What the caller sends, in order.
type Incoming = Vec<Result<CallerMessage, FrameError>>;
/// The fake installer file: its bytes and whether it was committed.
type SharedFile = std::rc::Rc<std::cell::RefCell<(Vec<u8>, bool)>>;

#[derive(Default)]
struct Link {
    incoming: VecDeque<Result<CallerMessage, FrameError>>,
    sent: Vec<HelperMessage>,
    /// Sends fail from this one on (the caller is gone).
    fail_sends_from: Option<usize>,
    waits: Vec<Duration>,
}

impl Link {
    fn with(messages: impl IntoIterator<Item = Result<CallerMessage, FrameError>>) -> Link {
        Link {
            incoming: messages.into_iter().collect(),
            ..Link::default()
        }
    }

    fn updates(&self) -> Vec<UpdateMessage> {
        self.sent
            .iter()
            .filter_map(|message| match message {
                HelperMessage::Update(update) => Some(update.clone()),
                _ => None,
            })
            .collect()
    }

    fn last_update(&self) -> Option<UpdateMessage> {
        self.updates().last().cloned()
    }

    fn sent_protocol_error(&self) -> bool {
        self.sent
            .iter()
            .any(|message| matches!(message, HelperMessage::Error(_)))
    }
}

impl StageLink for Link {
    fn send(&mut self, message: HelperMessage) -> Result<(), String> {
        if self
            .fail_sends_from
            .is_some_and(|from| self.sent.len() >= from)
        {
            return Err("the pipe was closed".to_string());
        }
        self.sent.push(message);
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<CallerMessage, FrameError> {
        self.waits.push(timeout);
        self.incoming.pop_front().unwrap_or(Err(FrameError::Closed))
    }
}

/// The chunks a well-behaved caller sends for `bytes`.
fn chunks(bytes: &[u8]) -> Vec<Result<CallerMessage, FrameError>> {
    bytes
        .chunks(CHUNK_LEN)
        .enumerate()
        .map(|(index, chunk)| {
            Ok(CallerMessage::InstallerChunk(InstallerChunk::new(
                (index * CHUNK_LEN) as u64,
                chunk,
            )))
        })
        .collect()
}

#[derive(Default)]
struct Sink {
    bytes: Vec<u8>,
    fail: bool,
}

impl StageSink for Sink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.fail {
            return Err("WriteFile failed with Win32 error 112".to_string());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

fn receive(link: &mut Link, sink: &mut Sink, bytes_in_plan: &[u8]) -> StagingEnd {
    let sha = mklm_ipc::encode_hex(&sha256_of(bytes_in_plan));
    receive_installer(
        link,
        sink,
        Stager::new(plan(bytes_in_plan.len() as u64, digest(&sha))),
        Duration::from_secs(30),
        Duration::from_secs(600),
        PROGRESS_EVERY,
    )
}

/// SHA-256 without a hashing crate here: `Sha256Stream` is mklm-update's.
fn sha256_of(bytes: &[u8]) -> [u8; 32] {
    let mut stream = mklm_update::Sha256Stream::new();
    stream.update(bytes);
    stream.finish().0
}

// ---- receive_installer ----

#[test]
fn receives_the_installer_in_order() {
    let bytes = installer();
    let mut link = Link::with(chunks(&bytes));
    let mut sink = Sink::default();
    let end = receive(&mut link, &mut sink, &bytes);
    assert_eq!(end, StagingEnd::Complete(digest(INSTALLER_SHA256)));
    assert_eq!(sink.bytes, bytes);
    let updates = link.updates();
    assert_eq!(
        updates[0],
        UpdateMessage::SendInstaller {
            name: "MKLM-Setup-0.2.1-x64.exe".to_string(),
            size: INSTALLER_SIZE as u64,
            sha256: INSTALLER_SHA256.to_string(),
            chunk_len: 65_536,
        }
    );
    // 1.2 MB: one progress report, when the first MiB is passed (after the 16th chunk).
    assert_eq!(
        updates[1..],
        [UpdateMessage::Received {
            bytes: 16 * CHUNK_LEN as u64
        }]
    );
    assert!(
        link.waits
            .iter()
            .all(|wait| *wait <= Duration::from_secs(30))
    );
}

#[test]
fn transfer_failures() {
    let bytes = installer();
    let cases: Vec<(&str, Incoming, StagingEnd, bool)> = vec![
        (
            "out of order",
            {
                let mut messages = chunks(&bytes);
                messages.swap(1, 2);
                messages
            },
            StagingEnd::Refused(UpdateRefusal::ChunkOutOfOrder {
                expected: CHUNK_LEN as u64,
                found: 2 * CHUNK_LEN as u64,
            }),
            false,
        ),
        (
            "Bye in the middle",
            {
                let mut messages = chunks(&bytes);
                messages.truncate(3);
                messages.push(Ok(CallerMessage::Bye));
                messages
            },
            StagingEnd::CallerLeft,
            false,
        ),
        (
            "the pipe closed",
            {
                let mut messages = chunks(&bytes);
                messages.truncate(3);
                messages.push(Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof)));
                messages
            },
            StagingEnd::CallerLeft,
            false,
        ),
        (
            "silence",
            {
                let mut messages = chunks(&bytes);
                messages.truncate(2);
                messages.push(Err(FrameError::Timeout));
                messages
            },
            StagingEnd::Refused(UpdateRefusal::CallerLeft),
            false,
        ),
        (
            "a request instead of a chunk",
            {
                let mut messages = chunks(&bytes);
                messages.truncate(2);
                messages.push(Ok(CallerMessage::Request(Request::Recover {
                    apply: mklm_ipc::ApplyOptions::default(),
                })));
                messages
            },
            StagingEnd::CallerLeft,
            true,
        ),
        (
            "a second stage request",
            vec![Ok(CallerMessage::StageUpdate(StageUpdateRequest {
                manifest: String::new(),
                signature: String::new(),
            }))],
            StagingEnd::CallerLeft,
            true,
        ),
        (
            "upper-case hex",
            vec![Ok(CallerMessage::InstallerChunk(InstallerChunk {
                offset: 0,
                hex: "4D5A".to_string(),
            }))],
            StagingEnd::Refused(UpdateRefusal::ChunkMalformed),
            false,
        ),
        (
            "an empty chunk",
            vec![Ok(CallerMessage::InstallerChunk(InstallerChunk {
                offset: 0,
                hex: String::new(),
            }))],
            StagingEnd::Refused(UpdateRefusal::ChunkMalformed),
            false,
        ),
        (
            "other bytes",
            {
                let mut other = bytes.clone();
                other[123_456] ^= 0x01;
                chunks(&other)
            },
            StagingEnd::Refused(UpdateRefusal::InstallerHashMismatch),
            false,
        ),
    ];
    for (label, messages, expected, protocol_error) in cases {
        let mut link = Link::with(messages);
        let mut sink = Sink::default();
        let end = receive(&mut link, &mut sink, &bytes);
        assert_eq!(end, expected, "{label}");
        if let StagingEnd::Refused(refusal) = &expected {
            assert_eq!(
                link.last_update(),
                Some(UpdateMessage::Refused(refusal.clone())),
                "{label}"
            );
        }
        assert_eq!(link.sent_protocol_error(), protocol_error, "{label}");
    }
}

#[test]
fn sizes_and_deadlines() {
    // More bytes than the manifest's size: refused at the chunk that goes past it.
    let small = vec![7u8; 100];
    let mut link = Link::with(chunks(&[7u8; 101]));
    let mut sink = Sink::default();
    assert!(matches!(
        receive(&mut link, &mut sink, &small),
        StagingEnd::Refused(UpdateRefusal::InstallerSizeMismatch { .. })
    ));
    assert!(sink.bytes.is_empty(), "nothing past the size is written");
    // A chunk of 64 KiB + 1: malformed (the pipe's frame would not even carry it).
    let mut link = Link::with([Ok(CallerMessage::InstallerChunk(InstallerChunk {
        offset: 0,
        hex: "00".repeat(CHUNK_LEN + 1),
    }))]);
    let big = vec![0u8; CHUNK_LEN + 1];
    assert_eq!(
        receive(&mut link, &mut Sink::default(), &big),
        StagingEnd::Refused(UpdateRefusal::ChunkMalformed)
    );
    // The whole transfer has a deadline.
    let bytes = installer();
    let mut link = Link::with(chunks(&bytes));
    let sha = INSTALLER_SHA256;
    let end = receive_installer(
        &mut link,
        &mut Sink::default(),
        Stager::new(plan(INSTALLER_SIZE as u64, digest(sha))),
        Duration::from_secs(30),
        Duration::ZERO,
        PROGRESS_EVERY,
    );
    assert_eq!(end, StagingEnd::Refused(UpdateRefusal::CallerLeft));
    // A write that fails.
    let mut link = Link::with(chunks(&bytes));
    let mut sink = Sink {
        fail: true,
        ..Sink::default()
    };
    assert_eq!(
        receive(&mut link, &mut sink, &bytes),
        StagingEnd::Refused(UpdateRefusal::Storage {
            detail: "WriteFile failed with Win32 error 112".to_string()
        })
    );
    // A caller that cannot even take the offer.
    let mut link = Link::with(chunks(&bytes));
    link.fail_sends_from = Some(0);
    assert_eq!(
        receive(&mut link, &mut Sink::default(), &bytes),
        StagingEnd::CallerLeft
    );
}

// ---- The fake machine of H1 ----

const BOOT: BootId = BootId::from_boot_counter(7);
const CALLER: u32 = 8532;
const STAGER: u32 = 9120;
const RUNNER: u32 = 9344;

fn process(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        creation_time: 134_041_234_000_000_000 + u64::from(pid),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunnerScript {
    /// Writes `ready` after this many polls.
    ReadyAfter(u32),
    /// Exits with this code after this many polls, never ready.
    ExitsAfter(u32, u32),
    NeverReady,
}

struct Fake {
    anchors: Option<TrustAnchors>,
    version: Version,
    in_install_dir: bool,
    free: Result<u64, String>,
    lock_result: Result<(), UpdateRefusal>,
    lock_held: bool,
    journal: Result<Journal, String>,
    alive: Vec<u32>,
    install_state: InstallState,
    trust: TrustState,
    run: Option<RunRecord>,
    last_result: Option<UpdateResult>,
    run_dir: Option<String>,
    files: Vec<(String, Vec<u8>)>,
    installer: Option<SharedFile>,
    fail: Option<&'static str>,
    runner: RunnerScript,
    spawned: bool,
    polls: u32,
    events: Vec<String>,
}

impl Fake {
    fn new(anchors: Option<TrustAnchors>) -> Fake {
        Fake {
            anchors,
            version: Version::new(0, 2, 0),
            in_install_dir: true,
            free: Ok(50 * 1024 * 1024 * 1024),
            lock_result: Ok(()),
            lock_held: false,
            journal: Ok(Journal::default()),
            alive: vec![CALLER, STAGER],
            install_state: InstallState::from_build_ids([
                Some("0.2.0+aa".to_string()),
                Some("0.2.0+aa".to_string()),
                Some("0.2.0+aa".to_string()),
            ]),
            trust: TrustState::default(),
            run: None,
            last_result: None,
            run_dir: None,
            files: Vec::new(),
            installer: None,
            fail: None,
            runner: RunnerScript::ReadyAfter(3),
            spawned: false,
            polls: 0,
            events: Vec::new(),
        }
    }

    fn event(&mut self, event: impl Into<String>) {
        self.events.push(event.into());
    }

    fn has(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == event)
    }

    fn position(&self, event: &str) -> usize {
        self.events
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no {event} in {:?}", self.events))
    }

    fn failing(&self, step: &str) -> Result<(), String> {
        if self.fail == Some(step) {
            Err(format!("{step} failed"))
        } else {
            Ok(())
        }
    }

    /// Nothing of a run is left: no folder, no `Run`, no lock.
    fn assert_clean(&self, label: &str) {
        assert_eq!(self.run_dir, None, "{label}: run folder");
        assert_eq!(self.run, None, "{label}: Run");
        assert!(!self.lock_held, "{label}: lock");
    }
}

struct FakeInstaller(SharedFile);

impl StageSink for FakeInstaller {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.borrow_mut().0.extend_from_slice(bytes);
        Ok(())
    }
}

impl StagedInstaller for FakeInstaller {
    fn commit(self: Box<Self>) -> Result<(), String> {
        self.0.borrow_mut().1 = true;
        Ok(())
    }
}

impl StagerEnv for Fake {
    fn version(&self) -> &Version {
        &self.version
    }
    fn arch(&self) -> Arch {
        Arch::X64
    }
    fn anchors(&self) -> &TrustAnchors {
        self.anchors.as_ref().expect("anchors")
    }
    fn me(&self) -> ProcessIdentity {
        process(STAGER)
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
    fn runs_from_install_dir(&mut self) -> bool {
        self.in_install_dir
    }
    fn caller(&mut self) -> (Option<ProcessIdentity>, Option<u32>) {
        (Some(process(CALLER)), Some(1))
    }
    fn free_space_program_data(&mut self) -> Result<u64, String> {
        self.free.clone()
    }
    fn acquire_lock(&mut self, timeout: Duration) -> Result<(), UpdateRefusal> {
        assert!(!self.lock_held, "the lock is taken once");
        self.lock_result.clone()?;
        self.event(format!("lock:{}", timeout.as_secs()));
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
    fn liveness(&self, process: &ProcessIdentity) -> Liveness {
        if self.alive.contains(&process.pid) {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    }
    fn read_install_state(&mut self) -> InstallState {
        self.install_state.clone()
    }
    fn read_trust(&mut self) -> TrustState {
        self.trust.clone()
    }
    fn write_trust(&mut self, trust: &TrustState) -> Result<(), String> {
        assert!(self.lock_held, "Trust is written under the lock");
        self.failing("write_trust")?;
        self.event("write_trust");
        self.trust = trust.clone();
        Ok(())
    }
    fn read_run(&mut self) -> Result<Option<RunRecord>, String> {
        if self.spawned {
            self.polls += 1;
            if let RunnerScript::ReadyAfter(polls) = self.runner
                && self.polls > polls
                && let Some(run) = self.run.as_mut()
                && run.phase == RunPhase::Staged
            {
                run.phase = RunPhase::Ready;
                run.runner = Some(process(RUNNER));
            }
        }
        Ok(self.run.clone())
    }
    fn write_run(&mut self, run: &RunRecord) -> Result<(), String> {
        assert!(self.lock_held, "Run is written under the lock");
        self.failing(if run.phase == RunPhase::Staging {
            "write_run staging"
        } else {
            "write_run staged"
        })?;
        self.event(format!("write_run:{:?}", run.phase));
        self.run = Some(run.clone());
        Ok(())
    }
    fn delete_run(&mut self) -> Result<(), String> {
        self.event("delete_run");
        self.run = None;
        Ok(())
    }
    fn write_last_result(&mut self, result: &UpdateResult) -> Result<(), String> {
        self.event("last_result");
        self.last_result = Some(result.clone());
        Ok(())
    }
    fn sweep_run_dirs(&mut self, keep: Option<&RunId>) {
        assert_eq!(keep, None);
        self.event("sweep");
    }
    fn random_suffix(&mut self) -> [u8; 8] {
        [1, 2, 3, 4, 5, 6, 7, 8]
    }
    fn create_run_dir(&mut self, run_id: &RunId) -> Result<(), String> {
        self.failing("create_run_dir")?;
        self.event("create_run_dir");
        self.run_dir = Some(run_id.as_str().to_string());
        Ok(())
    }
    fn remove_run_dir(&mut self, run_id: &RunId) {
        assert_eq!(self.run_dir.as_deref(), Some(run_id.as_str()));
        self.event("remove_run_dir");
        self.run_dir = None;
    }
    fn write_run_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        assert!(self.run_dir.is_some());
        self.failing("write_run_file")?;
        self.files.push((name.to_string(), bytes.to_vec()));
        Ok(())
    }
    fn create_installer(&mut self, name: &str) -> Result<Box<dyn StagedInstaller>, String> {
        assert_eq!(name, "MKLM-Setup-0.2.1-x64.exe");
        self.failing("create_installer")?;
        let file = std::rc::Rc::new(std::cell::RefCell::new((Vec::new(), false)));
        self.installer = Some(std::rc::Rc::clone(&file));
        Ok(Box::new(FakeInstaller(file)))
    }
    fn copy_self_as_runner(&mut self) -> Result<(), String> {
        self.failing("copy_self_as_runner")?;
        self.event("copy_runner");
        Ok(())
    }
    fn create_tmp_dir(&mut self) -> Result<(), String> {
        self.failing("create_tmp_dir")?;
        self.event("tmp");
        Ok(())
    }
    fn spawn_runner(&mut self, run_id: &RunId) -> Result<ProcessIdentity, String> {
        assert_eq!(
            self.run.as_ref().map(|run| run.phase),
            Some(RunPhase::Staged)
        );
        assert_eq!(self.run_dir.as_deref(), Some(run_id.as_str()));
        self.failing("spawn_runner")?;
        self.event("spawn");
        self.spawned = true;
        Ok(process(RUNNER))
    }
    fn runner_exit_code(&mut self) -> Option<u32> {
        match self.runner {
            RunnerScript::ExitsAfter(polls, code) if self.polls >= polls => Some(code),
            _ => None,
        }
    }
    fn stop_runner(&mut self, wait: Duration) {
        assert_eq!(wait, timing::RUNNER_KILL_WAIT);
        self.event("stop_runner");
    }
    fn sleep(&mut self, duration: Duration) {
        assert_eq!(duration, Duration::from_millis(250));
    }
    fn log(&mut self, _line: &str) {}
}

/// A journal with one entry (a recorded development-machine operation) in `state`.
fn journal_with(state: OpState) -> Journal {
    let json = include_str!(
        "../../mklm-core/testdata/journal/schema-1/bfaca7cd-fdef-4dd0-8d75-6f311d32bc37.json"
    );
    let mut entry = JournalEntry::from_json(json.trim()).expect("the recorded entry");
    entry.state = state;
    Journal {
        entries: vec![entry],
        ..Journal::default()
    }
}

fn request(manifest: &str, signature: &str) -> StageUpdateRequest {
    StageUpdateRequest {
        manifest: manifest.to_string(),
        signature: signature.to_string(),
    }
}

/// A caller that sends the installer when asked.
fn caller_link() -> Link {
    Link::with(chunks(&installer()))
}

fn refused_before_anything(label: &str, inject: impl FnOnce(&mut Fake), refusal: UpdateRefusal) {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    inject(&mut fake);
    let run_before = fake.run.clone();
    let mut link = caller_link();
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    assert_eq!(end, StageFlowEnd::Refused(refusal.clone()), "{label}");
    assert_eq!(link.updates(), [UpdateMessage::Refused(refusal)], "{label}");
    // Nothing of this run; a `Run` of another update stays as it was.
    assert_eq!(fake.run, run_before, "{label}: Run");
    assert_eq!(fake.run_dir, None, "{label}: run folder");
    assert!(!fake.lock_held, "{label}: lock");
    assert!(!fake.has("create_run_dir"), "{label}");
    assert!(!fake.has("spawn"), "{label}");
    assert_eq!(fake.last_result, None, "{label}");
}

// ---- stage_update ----

#[test]
fn stages_and_hands_off() {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    let mut link = caller_link();
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    let run_id = RunId::parse(RUN).unwrap();
    assert_eq!(
        end,
        StageFlowEnd::HandedOff {
            run_id: run_id.clone()
        }
    );
    // The messages of design m5b D.1.
    let updates = link.updates();
    assert!(matches!(updates[0], UpdateMessage::SendInstaller { .. }));
    assert_eq!(
        updates[1..],
        [
            UpdateMessage::Received {
                bytes: 16 * CHUNK_LEN as u64
            },
            UpdateMessage::StartingRunner,
            UpdateMessage::HandedOff {
                run_id: RUN.to_string(),
                to_version: "0.2.1".to_string(),
            },
        ]
    );
    // The run folder: exactly the bytes that verified, the committed installer.
    assert_eq!(
        fake.files,
        [
            ("latest.json".to_string(), MANIFEST_NEW.as_bytes().to_vec()),
            (
                "latest.json.minisig".to_string(),
                SIGNATURE_NEW.as_bytes().to_vec()
            ),
        ]
    );
    let (written, committed) = fake.installer.as_ref().unwrap().borrow().clone();
    assert!(committed);
    assert_eq!(written, installer());
    // Run: staging, then staged; H2 made it ready. Its fields.
    let run = fake.run.clone().unwrap();
    assert_eq!(run.run_id, run_id);
    assert_eq!(run.phase, RunPhase::Ready);
    assert_eq!(run.from_version, "0.2.0");
    assert_eq!(run.to_version, "0.2.1");
    assert_eq!(run.caller, Some(process(CALLER)));
    assert_eq!(run.caller_session, Some(1));
    assert_eq!(run.stager, process(STAGER));
    assert_eq!(run.boot_id, BOOT);
    // The boot as `StagerEnv::boot_id` gives it: the counter form (design m2 C.10), which the
    // engine's legacy rule never applies to.
    assert!(!run.boot_id.is_legacy());
    // The machine record moved to this manifest (design m5b D.4 step 9).
    assert_eq!(fake.trust.max_issued_at.get(KEY_ID), Some(&NEW_ISSUED_AT));
    // The lock from step 5 to the end; the order of the steps.
    let order = [
        "lock:10",
        "sweep",
        "write_trust",
        "create_run_dir",
        "write_run:Staging",
        "copy_runner",
        "tmp",
        "write_run:Staged",
        "spawn",
        "unlock",
    ];
    let positions: Vec<usize> = order.iter().map(|event| fake.position(event)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{:?}",
        fake.events
    );
    assert!(!fake.lock_held);
    assert_eq!(fake.last_result, None);
}

#[test]
fn refusals_before_the_run_folder() {
    refused_before_anything(
        "a development copy",
        |f| f.in_install_dir = false,
        UpdateRefusal::NotInstalledCopy,
    );
    refused_before_anything(
        "not newer",
        |f| f.version = Version::new(0, 2, 1),
        UpdateRefusal::NotNewer {
            offered: "0.2.1".to_string(),
            installed: "0.2.1".to_string(),
        },
    );
    refused_before_anything(
        "disk full",
        |f| f.free = Ok(1024),
        UpdateRefusal::DiskFull {
            needed: INSTALLER_SIZE as u64 + 16 * 1024 * 1024,
            available: 1024,
        },
    );
    refused_before_anything(
        "busy",
        |f| f.lock_result = Err(UpdateRefusal::Busy),
        UpdateRefusal::Busy,
    );
    let open = journal_with(OpState::AwaitingConfirm);
    refused_before_anything(
        "an operation waits for the user",
        move |f| f.journal = Ok(open),
        UpdateRefusal::OperationOpen {
            waiting_for_reboot: false,
        },
    );
    refused_before_anything(
        "journal unreadable",
        |f| f.journal = Err("RegOpenKeyExW".to_string()),
        UpdateRefusal::JournalUnreadable,
    );
    refused_before_anything(
        "Trust cannot be written",
        |f| f.fail = Some("write_trust"),
        UpdateRefusal::Storage {
            detail: "write_trust failed".to_string(),
        },
    );
    refused_before_anything(
        "the run folder cannot be made",
        |f| f.fail = Some("create_run_dir"),
        UpdateRefusal::Storage {
            detail: "create_run_dir failed".to_string(),
        },
    );
    // A tampered manifest: refused at step 3, before the lock.
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    let mut link = caller_link();
    let tampered = MANIFEST_NEW.replace("6029312", "6029313");
    let end = stage_update(&mut fake, &mut link, &request(&tampered, SIGNATURE_NEW));
    assert_eq!(end, StageFlowEnd::Refused(UpdateRefusal::BadSignature));
    assert!(!fake.has("lock:10"));
    assert_eq!(fake.trust, TrustState::default());
}

fn staging_record(phase: RunPhase, stager: u32) -> RunRecord {
    RunRecord {
        schema: 1,
        run_id: RunId::parse("0.2.1-ffffffffffffffff").unwrap(),
        from_version: "0.2.0".to_string(),
        to_version: "0.2.1".to_string(),
        arch: Arch::X64,
        phase,
        boot_id: BOOT,
        started_at: Timestamp(1),
        phase_at: Timestamp(2),
        caller: None,
        caller_session: None,
        stager: process(stager),
        runner: Some(process(4444)),
        installer: Some(process(5555)),
    }
}

#[test]
fn another_update_in_progress_or_interrupted() {
    // The installer of another run still runs (RELIABILITY-4).
    let mut installing = staging_record(RunPhase::Installing, 3333);
    installing.run_id = RunId::parse("0.2.1-eeeeeeeeeeeeeeee").unwrap();
    let left = installing.clone();
    refused_before_anything(
        "an installer still runs",
        move |f| {
            f.run = Some(left);
            f.alive.push(5555);
        },
        UpdateRefusal::UpdateInProgress,
    );
    // An interrupted one is moved to LastResult, then this run goes on.
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    fake.run = Some(installing.clone());
    let mut link = caller_link();
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    assert!(matches!(end, StageFlowEnd::HandedOff { .. }), "{end:?}");
    let result = fake
        .last_result
        .clone()
        .expect("the interruption is recorded");
    assert_eq!(result.run_id, installing.run_id);
    assert_eq!(
        result.outcome,
        UpdateOutcome::Interrupted {
            phase: RunPhase::Installing
        }
    );
    assert_eq!(result.installed_version.as_deref(), Some("0.2.0"));
    assert!(fake.position("last_result") < fake.position("create_run_dir"));
}

#[test]
fn failures_after_the_run_folder_remove_it() {
    let cases: Vec<(&str, Option<&'static str>, UpdateRefusal)> = vec![
        (
            "Run cannot be written",
            Some("write_run staging"),
            UpdateRefusal::Storage {
                detail: "write_run staging failed".to_string(),
            },
        ),
        (
            "latest.json cannot be written",
            Some("write_run_file"),
            UpdateRefusal::Storage {
                detail: "write_run_file failed".to_string(),
            },
        ),
        (
            "the installer file cannot be made",
            Some("create_installer"),
            UpdateRefusal::Storage {
                detail: "create_installer failed".to_string(),
            },
        ),
        (
            "the runner cannot be copied",
            Some("copy_self_as_runner"),
            UpdateRefusal::Storage {
                detail: "copy_self_as_runner failed".to_string(),
            },
        ),
        (
            "tmp cannot be made",
            Some("create_tmp_dir"),
            UpdateRefusal::Storage {
                detail: "create_tmp_dir failed".to_string(),
            },
        ),
        (
            "Run = staged cannot be written",
            Some("write_run staged"),
            UpdateRefusal::Storage {
                detail: "write_run staged failed".to_string(),
            },
        ),
        (
            "the runner does not start",
            Some("spawn_runner"),
            UpdateRefusal::HandOffFailed {
                detail: "spawn_runner failed".to_string(),
            },
        ),
    ];
    for (label, fail, refusal) in cases {
        let keys = anchors();
        let mut fake = Fake::new(Some(keys));
        fake.fail = fail;
        let mut link = caller_link();
        let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
        assert_eq!(end, StageFlowEnd::Refused(refusal.clone()), "{label}");
        assert_eq!(
            link.last_update(),
            Some(UpdateMessage::Refused(refusal)),
            "{label}"
        );
        fake.assert_clean(label);
        assert_eq!(fake.last_result, None, "{label}: no LastResult");
        assert!(fake.has("create_run_dir"), "{label}");
    }
}

#[test]
fn the_caller_leaves_during_the_transfer() {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    let mut messages = chunks(&installer());
    messages.truncate(5);
    messages.push(Ok(CallerMessage::Bye));
    let mut link = Link::with(messages);
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    assert_eq!(end, StageFlowEnd::CallerLeft);
    fake.assert_clean("Bye");
    assert_eq!(fake.last_result, None);
    let (_, committed) = fake.installer.as_ref().unwrap().borrow().clone();
    assert!(
        !committed,
        "an uncommitted installer file is deleted with its folder"
    );
    // Other bytes than the manifest's: refused, removed.
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    let mut other = installer();
    other[0] ^= 0x01;
    let mut link = Link::with(chunks(&other));
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    assert_eq!(
        end,
        StageFlowEnd::Refused(UpdateRefusal::InstallerHashMismatch)
    );
    fake.assert_clean("hash");
    assert!(!fake.has("copy_runner"));
}

#[test]
fn the_runner_that_does_not_get_ready() {
    // It ends before `ready`: its exit code is in the refusal; nothing needs stopping.
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    fake.runner = RunnerScript::ExitsAfter(2, 7);
    let mut link = caller_link();
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    let StageFlowEnd::Refused(UpdateRefusal::HandOffFailed { detail }) = end else {
        panic!("{end:?}");
    };
    assert!(detail.contains("code 7"), "{detail}");
    assert!(!fake.has("stop_runner"));
    fake.assert_clean("exited");
    assert!(matches!(
        link.updates()[..],
        [
            UpdateMessage::SendInstaller { .. },
            UpdateMessage::Received { .. },
            UpdateMessage::StartingRunner,
            UpdateMessage::Refused(UpdateRefusal::HandOffFailed { .. })
        ]
    ));
    // Not ready within 120 s: stopped and waited for first, then its folder and Run removed.
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    fake.runner = RunnerScript::NeverReady;
    let mut link = caller_link();
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    assert!(
        matches!(
            end,
            StageFlowEnd::Refused(UpdateRefusal::HandOffFailed { .. })
        ),
        "{end:?}"
    );
    assert!(fake.position("stop_runner") < fake.position("remove_run_dir"));
    assert!(fake.position("remove_run_dir") < fake.position("delete_run"));
    assert!(fake.position("delete_run") < fake.position("unlock"));
    fake.assert_clean("timeout");
    // 120 s of 250 ms polls.
    assert!(fake.polls >= 480, "{}", fake.polls);
}

#[test]
fn a_caller_gone_before_the_runner_starts_stops_the_run() {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    let mut link = caller_link();
    // SendInstaller and Received went out; StartingRunner cannot.
    link.fail_sends_from = Some(2);
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_NEW, SIGNATURE_NEW));
    assert_eq!(end, StageFlowEnd::CallerLeft);
    assert!(!fake.has("spawn"));
    fake.assert_clean("caller gone");
}

// ---- RecordTrust (SECURITY-5, FIX-VERIFICATION-6) ----

fn report(manifest: &str, signature: &str) -> TrustReport {
    TrustReport {
        manifest: manifest.to_string(),
        signature: signature.to_string(),
    }
}

#[test]
fn record_trust_moves_the_machine_record_once() {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    assert_eq!(
        record_trust(&mut fake, &report(MANIFEST_NEW, SIGNATURE_NEW)),
        UpdateMessage::TrustRecorded { changed: true }
    );
    assert_eq!(fake.trust.max_issued_at.get(KEY_ID), Some(&NEW_ISSUED_AT));
    assert!(fake.has("lock:2") && fake.has("unlock"));
    assert!(!fake.lock_held);
    // The same again: verified, nothing new, nothing written.
    fake.events.clear();
    assert_eq!(
        record_trust(&mut fake, &report(MANIFEST_NEW, SIGNATURE_NEW)),
        UpdateMessage::TrustRecorded { changed: false }
    );
    assert!(!fake.has("write_trust"));
    // An older manifest after it: a rollback, not recorded.
    let before = fake.trust.clone();
    assert!(matches!(
        record_trust(&mut fake, &report(MANIFEST_OLD, SIGNATURE_OLD)),
        UpdateMessage::TrustNotRecorded(UpdateRefusal::Rollback { .. })
    ));
    assert_eq!(fake.trust, before);
}

#[test]
fn record_trust_refusals_write_nothing() {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    let tampered = SIGNATURE_NEW.replace("issued_at=1792108800", "issued_at=1792108801");
    assert_eq!(
        record_trust(&mut fake, &report(MANIFEST_NEW, &tampered)),
        UpdateMessage::TrustNotRecorded(UpdateRefusal::BadSignature)
    );
    assert_eq!(fake.trust, TrustState::default());
    assert!(!fake.lock_held);
    // The lock is not free within 2 s.
    fake.lock_result = Err(UpdateRefusal::Busy);
    assert_eq!(
        record_trust(&mut fake, &report(MANIFEST_NEW, SIGNATURE_NEW)),
        UpdateMessage::TrustNotRecorded(UpdateRefusal::Busy)
    );
    assert_eq!(fake.trust, TrustState::default());
    assert!(!fake.has("write_trust"));
}

/// Design m5b F.2: after `record_trust(M2)`, a `stage_update` of the older, correctly signed M1
/// in the same session is a rollback, and makes neither a folder nor a `Run`.
#[test]
fn staging_an_older_manifest_after_record_trust_is_a_rollback() {
    let keys = anchors();
    let mut fake = Fake::new(Some(keys));
    assert_eq!(
        record_trust(&mut fake, &report(MANIFEST_NEW, SIGNATURE_NEW)),
        UpdateMessage::TrustRecorded { changed: true }
    );
    let mut link = caller_link();
    let end = stage_update(&mut fake, &mut link, &request(MANIFEST_OLD, SIGNATURE_OLD));
    assert_eq!(
        end,
        StageFlowEnd::Refused(UpdateRefusal::Rollback {
            issued_at: OLD_ISSUED_AT,
            seen: NEW_ISSUED_AT,
        })
    );
    fake.assert_clean("rollback");
    assert!(!fake.has("create_run_dir"));
}

// ---- CallerOrder (design m5b D.3) ----

fn record_trust_message() -> CallerMessage {
    CallerMessage::RecordTrust(report(MANIFEST_NEW, SIGNATURE_NEW))
}

fn a_request() -> CallerMessage {
    CallerMessage::Request(Request::Recover {
        apply: mklm_ipc::ApplyOptions::default(),
    })
}

fn stage_message() -> CallerMessage {
    CallerMessage::StageUpdate(request(MANIFEST_NEW, SIGNATURE_NEW))
}

#[test]
fn record_trust_only_first() {
    // First after Welcome: accepted, and whatever the answer, requests go on.
    let mut order = CallerOrder::new();
    assert_eq!(order.check(&record_trust_message()), Ok(()));
    assert_eq!(order.check(&a_request()), Ok(()));
    assert_eq!(order.check(&a_request()), Ok(()));
    assert_eq!(order.check(&stage_message()), Ok(()));
    assert_eq!(order.check(&stage_message()), Ok(()));
    assert_eq!(order.check(&CallerMessage::Bye), Ok(()));
    // After a request, a second one, after a stage request: protocol errors.
    for before in [a_request(), record_trust_message(), stage_message()] {
        let mut order = CallerOrder::new();
        assert_eq!(order.check(&before), Ok(()));
        assert!(order.check(&record_trust_message()).is_err(), "{before:?}");
    }
    // Chunks never reach the session loop legitimately; a second Welcome neither.
    let mut order = CallerOrder::new();
    assert!(
        order
            .check(&CallerMessage::InstallerChunk(InstallerChunk::new(0, &[0])))
            .is_err()
    );
    let mut order = CallerOrder::new();
    assert!(
        order
            .check(&CallerMessage::Welcome(Welcome::new(1, "0.1.0+x")))
            .is_err()
    );
    // Without RecordTrust, a session is what it always was.
    let mut order = CallerOrder::new();
    for message in [
        a_request(),
        stage_message(),
        a_request(),
        CallerMessage::Bye,
    ] {
        assert_eq!(order.check(&message), Ok(()));
    }
}

#[test]
fn the_fixture_is_what_it_claims() {
    // Independent of WP-U: the fixture's key ID is the one in the manifests and the public key.
    assert!(MANIFEST_OLD.contains(KEY_ID) && MANIFEST_NEW.contains(KEY_ID));
    assert!(MANIFEST_OLD.contains(INSTALLER_SHA256));
    assert!(SIGNATURE_OLD.contains("issued_at=1792022400"));
    assert!(SIGNATURE_NEW.contains("issued_at=1792108800"));
    assert_eq!(installer().len(), INSTALLER_SIZE);
    assert_eq!(timing::READY_WAIT, Duration::from_secs(120));
    assert_eq!(PROGRESS_EVERY, 1024 * 1024);
}
