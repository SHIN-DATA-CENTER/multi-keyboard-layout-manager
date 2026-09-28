//! The caller's side of an update session (design m5b D.3, E.4): `StageUpdate`, the installer in
//! chunks, then the hand-off.
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::path::Path;

use mklm_update::UpdateRefusal;

use crate::session::Link;
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
    StageEnd::Protocol("not implemented (m5b skeleton)".to_string()) // Skeleton (M5b): WP-C
}
