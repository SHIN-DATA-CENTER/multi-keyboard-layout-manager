//! Framing: a 4-byte little-endian length, then that many bytes of UTF-8 JSON of a [`Frame`].
//! Frames of length 0 or above [`MAX_FRAME_LEN`] are protocol errors that end the session.

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Largest accepted frame body. The largest real message (a migration plan with its events) is a
/// few tens of KiB.
pub const MAX_FRAME_LEN: u32 = 256 * 1024;

/// One message on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame<T> {
    /// [`crate::PROTOCOL_VERSION`] of the sender. A mismatch ends the session.
    pub v: u32,
    /// Per-direction sequence number starting at 1; a gap or repeat ends the session.
    pub seq: u64,
    pub body: T,
}

/// The envelope of a [`Frame`] without its body. [`read_frame`] parses this first (serde ignores
/// the body), checks `v`, and only then parses the body, so that a peer of another protocol
/// version is reported as [`FrameError::Version`] rather than as malformed JSON
/// (design review S11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct FrameHeader {
    pub v: u32,
    pub seq: u64,
}

/// Framing failed. Every variant ends the session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("the pipe was closed")]
    Closed,
    #[error("pipe I/O failed: {0:?}")]
    Io(std::io::ErrorKind),
    #[error("timed out waiting for a message")]
    Timeout,
    #[error("frame length {len} is outside 1..={max}")]
    BadLength { len: u32, max: u32 },
    #[error("malformed message: {0}")]
    Json(String),
    #[error("protocol version {found}, expected {expected}")]
    Version { found: u32, expected: u32 },
    #[error("sequence number {found}, expected {expected}")]
    Sequence { found: u64, expected: u64 },
}

/// Serializes and writes one frame (length and body in one `write_all`).
pub fn write_frame<W: Write, T: Serialize>(
    writer: &mut W,
    frame: &Frame<T>,
) -> Result<(), FrameError> {
    todo!("M2")
}

/// Reads one frame. Checks the length before allocating, then the version (from [`FrameHeader`],
/// before the body is parsed); the caller checks `seq`.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<Frame<T>, FrameError> {
    todo!("M2")
}
