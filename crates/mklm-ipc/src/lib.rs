//! Protocol between the unelevated MKLM caller (GUI or CLI) and `mklm-helper.exe` (plan 2.2).
//!
//! Pure Rust (no Windows calls): the pipe itself lives in `mklm-win::pipe`. This crate defines
//! - [`args`]: the helper's fixed command line (pipe name, nonce, caller PID) and its strict parser;
//! - [`frame`]: length-prefixed JSON framing with a size cap, the version check before the body
//!   is parsed, and per-direction sequence numbers ([`FrameSequencer`], [`FrameReader`]);
//! - [`message`]: the requests the pipe allows, the handshake frames, and (re-exported from
//!   `mklm_core::report`) events, decisions and results, versioned by [`PROTOCOL_VERSION`];
//! - [`handshake`]: the checks both sides make before the first request (version, build ID, nonce,
//!   PID).
//!
//! The caller creates the pipe (it is the pipe *server*); the helper connects as the client.
//! See section E of docs/design/m2-engine.md.

#![forbid(unsafe_code)]

pub mod args;
pub mod frame;
pub mod handshake;
pub mod message;

pub use args::*;
pub use frame::*;
pub use handshake::*;
pub use message::*;

/// Version of the message set. Caller and helper ship together, so they must match exactly; bump it
/// on any change to [`message`] or to a `mklm-core` type that messages embed (`mklm_core::report`
/// included). The build ID check (`Hello::build_id`) catches what a forgotten bump would miss.
///
/// 2 (M3, design m3 A.5): `Request::CleanupValues`, `Request::SetMachineSettings` and
/// `ApplyOptions::countdown_seconds`.
pub const PROTOCOL_VERSION: u32 = 2;
