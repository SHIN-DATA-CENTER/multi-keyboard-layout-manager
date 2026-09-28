//! The caller's side of an update session (design m5b D.3, E.4): `StageUpdate`, the installer in
//! chunks, then the hand-off.
//!
//! Over a connected, verified helper link (H1, launched with the UAC prompt by the front end):
//! 1. `RecordTrust` when the user's record is ahead of the machine's (C.4; the reporter the front
//!    end installed, `session::send_trust_report`), then `StageUpdate` with the manifest and the
//!    signature that verified, exactly as received.
//! 2. H1 verifies on its own and answers `SendInstaller` (or `Refused`). What it asks for must be
//!    what the offer says — name, size, SHA-256, chunk length — or the session ends.
//! 3. The cached installer in pieces of at most `CHUNK_LEN`, its SHA-256 computed on the way; the
//!    last piece is held back when the digest does not match the manifest (the file changed since
//!    the download): `SourceChanged`, and H1 never has a complete file. The user may cancel until
//!    the last piece is sent.
//! 4. `StartingRunner`, heartbeats, then `HandedOff`: the caller must quit (E.4). A helper lost
//!    after the last piece is asked about through the machine record (`HandOffProbe`) for up to
//!    `CALLER_RUN_POLL` (RELIABILITY-5): if the runner took over for this process, it is a
//!    hand-off all the same.
//!
//! `Bye` is sent for every end but `HandedOff` (after which the helper exits by itself).

use std::fs::File;
use std::io::Read as _;
use std::path::Path;
use std::time::{Duration, Instant};

use mklm_ipc::{
    CHUNK_LEN, CallerMessage, HelperMessage, InstallerChunk, MESSAGE_TIMEOUT, StageUpdateRequest,
    UpdateMessage,
};
use mklm_update::run::timing::{CALLER_RUN_POLL, HANDOFF_WAIT};
use mklm_update::{Sha256Stream, UpdateRefusal};

use crate::session::{Link, Recv, send_trust_report};
use crate::update::check::Offer;

pub trait StageFrontend {
    fn sent(&mut self, bytes: u64, total: u64);
    fn received(&mut self, bytes: u64);
    /// `UpdateMessage::StartingRunner` arrived.
    fn starting_runner(&mut self);
    /// Honoured until the last chunk is sent.
    fn cancel_requested(&mut self) -> bool;
}

/// Asks the machine record whether this process was handed off although `HandedOff` was lost
/// (design m5b E.4; RELIABILITY-5): `Run.caller` is this process and `phase` is ready or waiting
/// with a live runner. `status::RunRecordProbe` reads HKLM.
pub trait HandOffProbe {
    fn handed_off(&mut self) -> Option<(String, String)>; // (run_id, to_version)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageEnd {
    /// The caller must quit now (design m5b E.4).
    HandedOff {
        run_id: String,
        to_version: String,
    },
    Refused(UpdateRefusal),
    Cancelled,
    /// The cached installer changed while it was sent (SHA-256 differs).
    SourceChanged,
    Lost(String),
    Unresponsive,
    Protocol(String),
}

/// Over a connected helper link: `StageUpdate`, then the installer in `CHUNK_LEN` pieces
/// (design m5b D.3), then waits up to `HANDOFF_WAIT`; after the last chunk a lost helper is
/// checked against `probe` for up to `CALLER_RUN_POLL`. Sends `Bye` itself except after
/// `HandedOff`.
pub fn stage(
    link: &mut dyn Link,
    offer: &Offer,
    installer: &Path,
    frontend: &mut dyn StageFrontend,
    probe: &mut dyn HandOffProbe,
) -> StageEnd {
    stage_with(
        link,
        offer,
        installer,
        frontend,
        probe,
        &StageTiming::default(),
    )
}

/// The waits of [`stage`] (tests shorten them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StageTiming {
    /// Silence after which a running helper counts as wedged (it sends a heartbeat every 10 s
    /// while it works, design m5b D.3).
    pub silence: Duration,
    /// After the last chunk, the longest wait for `HandedOff`.
    pub handoff: Duration,
    /// How long, and how often, the machine record is read after a lost helper.
    pub poll: Duration,
    pub poll_every: Duration,
    /// One wait for a frame.
    pub tick: Duration,
}

impl Default for StageTiming {
    fn default() -> StageTiming {
        StageTiming {
            silence: MESSAGE_TIMEOUT,
            handoff: HANDOFF_WAIT,
            poll: CALLER_RUN_POLL,
            poll_every: Duration::from_secs(1),
            tick: Duration::from_millis(200),
        }
    }
}

/// How waiting for a message ended.
enum Wait {
    Message(UpdateMessage),
    Lost(String),
    Unresponsive,
    Protocol(String),
}

/// [`stage`] with explicit waits.
pub(crate) fn stage_with(
    link: &mut dyn Link,
    offer: &Offer,
    installer: &Path,
    frontend: &mut dyn StageFrontend,
    probe: &mut dyn HandOffProbe,
    timing: &StageTiming,
) -> StageEnd {
    let end = run(link, offer, installer, frontend, probe, timing);
    if !matches!(end, StageEnd::HandedOff { .. }) {
        let _ = link.send(&CallerMessage::Bye);
    }
    end
}

fn run(
    link: &mut dyn Link,
    offer: &Offer,
    installer: &Path,
    frontend: &mut dyn StageFrontend,
    probe: &mut dyn HandOffProbe,
    timing: &StageTiming,
) -> StageEnd {
    let (Ok(manifest), Ok(signature)) = (
        String::from_utf8(offer.manifest.clone()),
        String::from_utf8(offer.signature.clone()),
    ) else {
        return StageEnd::Protocol("the verified update information is not UTF-8".into());
    };
    let mut trust_pending = match send_trust_report(link) {
        Ok(sent) => sent,
        Err(detail) => return StageEnd::Lost(detail),
    };
    if let Err(detail) = link.send(&CallerMessage::StageUpdate(StageUpdateRequest {
        manifest,
        signature,
    })) {
        return StageEnd::Lost(detail);
    }
    // H1 verifies, takes the lock and prepares its folder; it answers what to send.
    let (size, chunk_len) = match wait(link, timing, timing.silence, &mut trust_pending) {
        Wait::Message(UpdateMessage::SendInstaller {
            name,
            size,
            sha256,
            chunk_len,
        }) => {
            let asset = &offer.verified.asset;
            let chunk_len = usize::try_from(chunk_len).unwrap_or(0);
            if name != asset.name
                || size != asset.size
                || sha256 != asset.sha256.to_hex()
                || chunk_len == 0
                || chunk_len > CHUNK_LEN
            {
                return StageEnd::Protocol(format!(
                    "the helper asked for {name} ({size} bytes, SHA-256 {sha256}, pieces of \
                     {chunk_len}), not the offered {} ({} bytes)",
                    asset.name, asset.size
                ));
            }
            (size, chunk_len)
        }
        Wait::Message(UpdateMessage::Refused(refusal)) => return StageEnd::Refused(refusal),
        Wait::Message(other) => {
            return StageEnd::Protocol(format!("unexpected before the installer: {other:?}"));
        }
        Wait::Lost(detail) => return StageEnd::Lost(detail),
        Wait::Unresponsive => return StageEnd::Unresponsive,
        Wait::Protocol(detail) => return StageEnd::Protocol(detail),
    };
    if let Some(end) = send_installer(
        link,
        offer,
        installer,
        (size, chunk_len),
        frontend,
        &mut trust_pending,
    ) {
        return end;
    }
    // Every byte is with H1: it checks them, starts the runner and waits for it (up to 120 s,
    // with heartbeats).
    let deadline = Instant::now() + timing.handoff;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let lost = match wait(link, timing, timing.silence.min(left), &mut trust_pending) {
            Wait::Message(UpdateMessage::Received { bytes }) => {
                frontend.received(bytes);
                continue;
            }
            Wait::Message(UpdateMessage::StartingRunner) => {
                frontend.starting_runner();
                continue;
            }
            Wait::Message(UpdateMessage::HandedOff { run_id, to_version }) => {
                return StageEnd::HandedOff { run_id, to_version };
            }
            Wait::Message(UpdateMessage::Refused(refusal)) => return StageEnd::Refused(refusal),
            Wait::Message(other) => {
                return StageEnd::Protocol(format!("unexpected after the installer: {other:?}"));
            }
            Wait::Protocol(detail) => return StageEnd::Protocol(detail),
            Wait::Lost(detail) => StageEnd::Lost(detail),
            Wait::Unresponsive => StageEnd::Unresponsive,
        };
        // The helper is gone or silent after the last chunk: the runner may have taken over
        // before H1 could say so (RELIABILITY-5).
        let asked_until = Instant::now() + timing.poll;
        loop {
            if let Some((run_id, to_version)) = probe.handed_off() {
                return StageEnd::HandedOff { run_id, to_version };
            }
            if Instant::now() >= asked_until {
                return lost;
            }
            std::thread::sleep(timing.poll_every);
        }
    }
}

/// Sends the installer (step 3): `size` bytes in pieces of `chunk_len`. `None` when every byte
/// was sent.
fn send_installer(
    link: &mut dyn Link,
    offer: &Offer,
    installer: &Path,
    (size, chunk_len): (u64, usize),
    frontend: &mut dyn StageFrontend,
    trust_pending: &mut bool,
) -> Option<StageEnd> {
    let mut file = match File::open(installer) {
        Ok(file) => file,
        Err(_) => return Some(StageEnd::SourceChanged),
    };
    let mut hash = Sha256Stream::new();
    let mut offset = 0u64;
    let mut buffer = vec![0u8; chunk_len];
    while offset < size {
        if frontend.cancel_requested() {
            return Some(StageEnd::Cancelled);
        }
        let want = usize::try_from(size - offset).map_or(chunk_len, |left| left.min(chunk_len));
        if read_full(&mut file, &mut buffer[..want]).is_err() {
            // Shorter than the verified size: not the file that was downloaded.
            return Some(StageEnd::SourceChanged);
        }
        let piece = &buffer[..want];
        hash.update(piece);
        let last = offset + want as u64 == size;
        if last {
            let digest = std::mem::take(&mut hash).finish();
            if digest != offer.verified.asset.sha256 {
                return Some(StageEnd::SourceChanged);
            }
        }
        if let Err(detail) = link.send(&CallerMessage::InstallerChunk(InstallerChunk::new(
            offset, piece,
        ))) {
            return Some(StageEnd::Lost(detail));
        }
        offset += want as u64;
        frontend.sent(offset, size);
        // What H1 said meanwhile (progress, or a refusal of a piece). After the last piece the
        // wait for the hand-off reads on, and a helper lost then is asked about.
        if !last && let Some(end) = drain(link, frontend, trust_pending) {
            return Some(end);
        }
    }
    None
}

/// Reads what arrived without waiting (between pieces).
fn drain(
    link: &mut dyn Link,
    frontend: &mut dyn StageFrontend,
    trust_pending: &mut bool,
) -> Option<StageEnd> {
    loop {
        match link.recv(Duration::ZERO) {
            Recv::Timeout => return None,
            Recv::Closed(detail) => return Some(StageEnd::Lost(detail)),
            Recv::Message(message) => match classify_frame(message, trust_pending) {
                Frame::Skip => {}
                Frame::Update(UpdateMessage::Received { bytes }) => frontend.received(bytes),
                Frame::Update(UpdateMessage::Refused(refusal)) => {
                    return Some(StageEnd::Refused(refusal));
                }
                Frame::Update(other) => {
                    return Some(StageEnd::Protocol(format!(
                        "unexpected while sending the installer: {other:?}"
                    )));
                }
                Frame::Protocol(detail) => return Some(StageEnd::Protocol(detail)),
            },
        }
    }
}

/// A frame of an update session.
enum Frame {
    /// A heartbeat, or the answer to `RecordTrust`.
    Skip,
    Update(UpdateMessage),
    Protocol(String),
}

fn classify_frame(message: HelperMessage, trust_pending: &mut bool) -> Frame {
    match message {
        // Only heartbeats are expected; another event is ignored too (D.3).
        HelperMessage::Event(_) => Frame::Skip,
        HelperMessage::Update(
            reply @ (UpdateMessage::TrustRecorded { .. } | UpdateMessage::TrustNotRecorded(_)),
        ) if *trust_pending => {
            *trust_pending = false;
            crate::session::trust_answered(&reply);
            Frame::Skip
        }
        HelperMessage::Update(message) => Frame::Update(message),
        HelperMessage::Error(info) => {
            Frame::Protocol(format!("the helper reported an error: {}", info.message))
        }
        HelperMessage::Hello(_) | HelperMessage::Result(_) => {
            Frame::Protocol("unexpected frame in an update session".into())
        }
    }
}

/// The next update message within `silence` (heartbeats reset it).
fn wait(
    link: &mut dyn Link,
    timing: &StageTiming,
    silence: Duration,
    trust_pending: &mut bool,
) -> Wait {
    let mut last_frame = Instant::now();
    loop {
        match link.recv(timing.tick) {
            Recv::Message(message) => {
                last_frame = Instant::now();
                match classify_frame(message, trust_pending) {
                    Frame::Skip => {}
                    Frame::Update(message) => return Wait::Message(message),
                    Frame::Protocol(detail) => return Wait::Protocol(detail),
                }
            }
            Recv::Closed(detail) => return Wait::Lost(detail),
            Recv::Timeout => {
                if let Some(code) = link.helper_exit_code() {
                    return Wait::Lost(format!("the helper exited with code {code}"));
                }
                if last_frame.elapsed() >= silence {
                    return Wait::Unresponsive;
                }
            }
        }
    }
}

/// Fills `buffer` from `file`; an error when the file ends first.
fn read_full(file: &mut File, buffer: &mut [u8]) -> std::io::Result<()> {
    file.read_exact(buffer)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;

    use mklm_core::Event;
    use mklm_update::{OfferKind, SignatureSlot, Version};

    use super::*;
    use crate::update::cache::tests::Scratch;
    use crate::update::check::tests::{installer, verified};

    fn fast() -> StageTiming {
        StageTiming {
            silence: Duration::from_millis(100),
            handoff: Duration::from_millis(300),
            poll: Duration::from_millis(30),
            poll_every: Duration::from_millis(5),
            tick: Duration::from_millis(1),
        }
    }

    /// Plays H1: answers `StageUpdate` and the pieces from a script.
    #[derive(Debug, Default)]
    struct FakeH1 {
        /// After `StageUpdate`.
        on_stage: VecDeque<HelperMessage>,
        /// After the last piece.
        on_last: VecDeque<HelperMessage>,
        /// After the piece at this offset.
        on_piece: Option<(u64, HelperMessage)>,
        queue: VecDeque<HelperMessage>,
        sent: Vec<CallerMessage>,
        received: Vec<u8>,
        size: u64,
        closed_after_last: bool,
        closed: bool,
    }

    impl Link for FakeH1 {
        fn send(&mut self, message: &CallerMessage) -> Result<(), String> {
            if self.closed {
                return Err("closed".into());
            }
            self.sent.push(message.clone());
            match message {
                CallerMessage::StageUpdate(_) => self.queue.extend(self.on_stage.drain(..)),
                CallerMessage::InstallerChunk(chunk) => {
                    assert_eq!(chunk.offset, self.received.len() as u64);
                    self.received.extend(chunk.decode().unwrap());
                    if let Some((offset, _)) = &self.on_piece
                        && *offset == chunk.offset
                    {
                        let (_, message) = self.on_piece.take().unwrap();
                        self.queue.push_back(message);
                    }
                    if self.received.len() as u64 == self.size {
                        self.queue.extend(self.on_last.drain(..));
                        self.closed = self.closed_after_last;
                    }
                }
                _ => {}
            }
            Ok(())
        }

        fn recv(&mut self, _timeout: Duration) -> Recv {
            match self.queue.pop_front() {
                Some(message) => Recv::Message(message),
                None if self.closed => Recv::Closed("the helper closed the connection".into()),
                None => Recv::Timeout,
            }
        }

        fn helper_exit_code(&mut self) -> Option<u32> {
            None
        }
    }

    #[derive(Debug, Default)]
    struct Recorder {
        sent: Vec<(u64, u64)>,
        received: Vec<u64>,
        starting: u32,
        cancel_after: Option<u64>,
    }

    impl StageFrontend for Recorder {
        fn sent(&mut self, bytes: u64, total: u64) {
            self.sent.push((bytes, total));
        }

        fn received(&mut self, bytes: u64) {
            self.received.push(bytes);
        }

        fn starting_runner(&mut self) {
            self.starting += 1;
        }

        fn cancel_requested(&mut self) -> bool {
            self.cancel_after
                .is_some_and(|after| self.sent.last().is_some_and(|(sent, _)| *sent >= after))
        }
    }

    #[derive(Debug, Default)]
    struct Probe(Option<(String, String)>, u32);

    impl HandOffProbe for Probe {
        fn handed_off(&mut self) -> Option<(String, String)> {
            self.1 += 1;
            self.0.clone()
        }
    }

    const RUN: &str = "0.2.1-3f9a0c2b7d1e4a65";

    fn update(message: UpdateMessage) -> HelperMessage {
        HelperMessage::Update(message)
    }

    /// The offer, its installer in a scratch cache, and H1 that asks for exactly it.
    fn scene(tag: &str) -> (Scratch, Offer, std::path::PathBuf, FakeH1) {
        let scratch = Scratch::new(tag);
        let version = Version::new(0, 2, 1);
        let (bytes, asset) = installer(&version);
        fs::create_dir_all(&scratch.0).unwrap();
        let path = scratch.cache().installer_path(&asset.name);
        fs::write(&path, &bytes).unwrap();
        let offer = Offer {
            verified: verified("0.2.1", OfferKind::Newer),
            manifest: b"{\"version\":\"0.2.1\"}".to_vec(),
            signature: b"untrusted comment: x".to_vec(),
            slot: SignatureSlot::Main,
            skipped: false,
            downloaded: Some(path.clone()),
        };
        let h1 = FakeH1 {
            on_stage: [
                HelperMessage::Event(Event::Heartbeat),
                update(UpdateMessage::SendInstaller {
                    name: asset.name.clone(),
                    size: asset.size,
                    sha256: asset.sha256.to_hex(),
                    chunk_len: CHUNK_LEN as u32,
                }),
            ]
            .into(),
            size: asset.size,
            ..FakeH1::default()
        };
        (scratch, offer, path, h1)
    }

    fn go(
        h1: &mut FakeH1,
        offer: &Offer,
        path: &Path,
        frontend: &mut Recorder,
        probe: &mut Probe,
    ) -> StageEnd {
        stage_with(h1, offer, path, frontend, probe, &fast())
    }

    #[test]
    fn the_installer_is_sent_and_handed_off() {
        let (_scratch, offer, path, mut h1) = scene("stage-ok");
        let bytes = fs::read(&path).unwrap();
        h1.on_piece = Some((65_536, update(UpdateMessage::Received { bytes: 131_072 })));
        h1.on_last = [
            update(UpdateMessage::StartingRunner),
            HelperMessage::Event(Event::Heartbeat),
            update(UpdateMessage::HandedOff {
                run_id: RUN.into(),
                to_version: "0.2.1".into(),
            }),
        ]
        .into();
        let mut frontend = Recorder::default();
        let end = go(&mut h1, &offer, &path, &mut frontend, &mut Probe::default());
        assert_eq!(
            end,
            StageEnd::HandedOff {
                run_id: RUN.into(),
                to_version: "0.2.1".into()
            }
        );
        assert_eq!(h1.received, bytes);
        assert!(matches!(
            &h1.sent[0],
            CallerMessage::StageUpdate(request) if request.manifest == "{\"version\":\"0.2.1\"}"
        ));
        // No Bye after a hand-off.
        assert!(!h1.sent.contains(&CallerMessage::Bye));
        assert_eq!(frontend.sent.last(), Some(&(150_000, 150_000)));
        assert_eq!(frontend.received, vec![131_072]);
        assert_eq!(frontend.starting, 1);
    }

    #[test]
    fn a_refusal_ends_the_session_with_bye() {
        let (_scratch, offer, path, mut h1) = scene("stage-refused");
        h1.on_stage = [update(UpdateMessage::Refused(
            UpdateRefusal::OperationOpen {
                waiting_for_reboot: true,
            },
        ))]
        .into();
        let end = go(
            &mut h1,
            &offer,
            &path,
            &mut Recorder::default(),
            &mut Probe::default(),
        );
        assert_eq!(
            end,
            StageEnd::Refused(UpdateRefusal::OperationOpen {
                waiting_for_reboot: true
            })
        );
        assert_eq!(h1.sent.last(), Some(&CallerMessage::Bye));
        assert!(h1.received.is_empty());
        // Refused after the last piece (the hash, the runner).
        let (_scratch, offer, path, mut h1) = scene("stage-refused-late");
        h1.on_last = [update(UpdateMessage::Refused(
            UpdateRefusal::HandOffFailed {
                detail: "exit code 7".into(),
            },
        ))]
        .into();
        assert!(matches!(
            go(
                &mut h1,
                &offer,
                &path,
                &mut Recorder::default(),
                &mut Probe::default()
            ),
            StageEnd::Refused(UpdateRefusal::HandOffFailed { .. })
        ));
    }

    #[test]
    fn the_user_may_cancel_until_the_last_piece() {
        let (_scratch, offer, path, mut h1) = scene("stage-cancel");
        let mut frontend = Recorder {
            cancel_after: Some(65_536),
            ..Recorder::default()
        };
        let end = go(&mut h1, &offer, &path, &mut frontend, &mut Probe::default());
        assert_eq!(end, StageEnd::Cancelled);
        assert_eq!(h1.received.len(), 65_536);
        assert_eq!(h1.sent.last(), Some(&CallerMessage::Bye));
    }

    #[test]
    fn a_changed_file_is_never_completed() {
        let (_scratch, offer, path, mut h1) = scene("stage-changed");
        // Same size, other bytes at the end.
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        // The stub digest of the skeleton is all zeros: another digest for the offer makes the
        // mismatch visible whatever the digest implementation.
        let mut offer = offer;
        offer.verified.asset.sha256 = mklm_update::Sha256Digest([0xAA; 32]);
        h1.on_stage = [update(UpdateMessage::SendInstaller {
            name: offer.verified.asset.name.clone(),
            size: offer.verified.asset.size,
            sha256: offer.verified.asset.sha256.to_hex(),
            chunk_len: CHUNK_LEN as u32,
        })]
        .into();
        let end = go(
            &mut h1,
            &offer,
            &path,
            &mut Recorder::default(),
            &mut Probe::default(),
        );
        assert_eq!(end, StageEnd::SourceChanged);
        // Everything but the last piece.
        assert_eq!(h1.received.len(), 131_072);
        assert_eq!(h1.sent.last(), Some(&CallerMessage::Bye));
        // A file that became shorter.
        let (_scratch, offer, path, mut h1) = scene("stage-short");
        fs::write(&path, b"short").unwrap();
        assert_eq!(
            go(
                &mut h1,
                &offer,
                &path,
                &mut Recorder::default(),
                &mut Probe::default()
            ),
            StageEnd::SourceChanged
        );
    }

    #[test]
    fn a_request_for_another_file_ends_the_session() {
        let (_scratch, offer, path, mut h1) = scene("stage-other");
        h1.on_stage = [update(UpdateMessage::SendInstaller {
            name: "MKLM-Setup-0.2.1-x64.exe".replace("0.2.1", "9.9.9"),
            size: offer.verified.asset.size,
            sha256: offer.verified.asset.sha256.to_hex(),
            chunk_len: CHUNK_LEN as u32,
        })]
        .into();
        assert!(matches!(
            go(
                &mut h1,
                &offer,
                &path,
                &mut Recorder::default(),
                &mut Probe::default()
            ),
            StageEnd::Protocol(_)
        ));
        assert!(h1.received.is_empty());
        let (_scratch, offer, path, mut h1) = scene("stage-chunk");
        h1.on_stage = [update(UpdateMessage::SendInstaller {
            name: offer.verified.asset.name.clone(),
            size: offer.verified.asset.size,
            sha256: offer.verified.asset.sha256.to_hex(),
            chunk_len: CHUNK_LEN as u32 + 1,
        })]
        .into();
        assert!(matches!(
            go(
                &mut h1,
                &offer,
                &path,
                &mut Recorder::default(),
                &mut Probe::default()
            ),
            StageEnd::Protocol(_)
        ));
    }

    /// Lost after the last piece: the machine record decides (RELIABILITY-5).
    #[test]
    fn a_helper_lost_after_the_last_piece_is_asked_about() {
        let (_scratch, offer, path, mut h1) = scene("stage-lost");
        h1.closed_after_last = true;
        let mut probe = Probe(Some((RUN.into(), "0.2.1".into())), 0);
        assert_eq!(
            go(&mut h1, &offer, &path, &mut Recorder::default(), &mut probe),
            StageEnd::HandedOff {
                run_id: RUN.into(),
                to_version: "0.2.1".into()
            }
        );
        let (_scratch, offer, path, mut h1) = scene("stage-lost-really");
        h1.closed_after_last = true;
        let mut probe = Probe::default();
        assert!(matches!(
            go(&mut h1, &offer, &path, &mut Recorder::default(), &mut probe),
            StageEnd::Lost(_)
        ));
        assert!(probe.1 >= 2, "asked {} times", probe.1);
        // Silent after the last piece, beyond the silence limit: unresponsive, after asking.
        let (_scratch, offer, path, mut h1) = scene("stage-silent");
        let mut probe = Probe::default();
        assert_eq!(
            go(&mut h1, &offer, &path, &mut Recorder::default(), &mut probe),
            StageEnd::Unresponsive
        );
        assert!(probe.1 >= 1);
    }

    /// Heartbeats keep the wait for the runner alive (RELIABILITY-5).
    #[test]
    fn heartbeats_while_waiting_for_the_runner() {
        let (_scratch, offer, path, mut h1) = scene("stage-heartbeats");
        let mut last: VecDeque<HelperMessage> = [update(UpdateMessage::StartingRunner)].into();
        for _ in 0..5 {
            last.push_back(HelperMessage::Event(Event::Heartbeat));
        }
        last.push_back(update(UpdateMessage::HandedOff {
            run_id: RUN.into(),
            to_version: "0.2.1".into(),
        }));
        h1.on_last = last;
        let mut frontend = Recorder::default();
        assert!(matches!(
            go(&mut h1, &offer, &path, &mut frontend, &mut Probe::default()),
            StageEnd::HandedOff { .. }
        ));
        assert_eq!(frontend.starting, 1);
    }
}
