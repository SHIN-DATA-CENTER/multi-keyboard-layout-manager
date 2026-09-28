//! Protocol between the unelevated MKLM caller (GUI or CLI) and `mklm-helper.exe` (plan 2.2).
//!
//! Pure Rust (no Windows calls): the pipe itself lives in `mklm-win::pipe`. This crate defines
//! - [`args`]: the helper's fixed command line (pipe name, nonce, caller PID) and its strict parser;
//! - [`frame`]: length-prefixed JSON framing with a size cap, the version check before the body
//!   is parsed, and per-direction sequence numbers ([`FrameSequencer`], [`FrameReader`]);
//! - [`message`]: the requests the pipe allows, the handshake frames, and (re-exported from
//!   `mklm_core::report`) events, decisions and results, versioned by [`PROTOCOL_VERSION`];
//! - [`handshake`]: the checks both sides make before the first request (version, build ID, nonce,
//!   PID);
//! - [`update`]: the update messages (M5b, design m5b D.3), and [`staging`]: the helper's pure
//!   drivers of an update session (`stage_update`, `record_trust`) over environment traits.
//!
//! The caller creates the pipe (it is the pipe *server*); the helper connects as the client.
//! See section E of docs/design/m2-engine.md and section D of docs/design/m5b-updater.md.

#![forbid(unsafe_code)]

pub mod args;
pub mod frame;
pub mod handshake;
pub mod message;
pub mod staging;
pub mod update;

pub use args::*;
pub use frame::*;
pub use handshake::*;
pub use message::*;
pub use update::*;

/// Version of the message set. Caller and helper ship together, so they must match exactly; bump it
/// on any change to [`message`] or to a `mklm-core` / `mklm-update` type that messages embed
/// (`mklm_core::report` and `mklm_update::UpdateRefusal` included). The build ID check
/// (`Hello::build_id`) catches what a forgotten bump would miss.
///
/// 2 (M3, design m3 A.5): `Request::CleanupValues`, `Request::SetMachineSettings` and
/// `ApplyOptions::countdown_seconds`.
///
/// 3 (M5b, design m5b D.3): `CallerMessage::{RecordTrust, StageUpdate, InstallerChunk}`,
/// `HelperMessage::Update`.
pub const PROTOCOL_VERSION: u32 = 3;
