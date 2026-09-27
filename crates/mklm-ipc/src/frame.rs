//! Framing: a 4-byte little-endian length, then that many bytes of UTF-8 JSON of a [`Frame`].
//! Frames of length 0 or above [`MAX_FRAME_LEN`] are protocol errors that end the session.
//!
//! Each direction numbers its frames 1, 2, 3, … ([`FrameSequencer`] on the sending side); the
//! receiving side ([`FrameReader`]) ends the session on a gap or a repeat (section E.5).

use std::io::{ErrorKind, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;

/// Largest accepted frame body. The largest real message (a migration plan with its events) is a
/// few tens of KiB.
pub const MAX_FRAME_LEN: u32 = 256 * 1024;

/// Size of the length prefix.
const LEN_PREFIX: usize = 4;

/// One message on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame<T> {
    /// [`crate::PROTOCOL_VERSION`] of the sender. A mismatch ends the session.
    pub v: u32,
    /// Per-direction sequence number starting at 1; a gap or repeat ends the session.
    pub seq: u64,
    pub body: T,
}

impl<T> Frame<T> {
    /// A frame of this build's [`PROTOCOL_VERSION`].
    pub fn new(seq: u64, body: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            seq,
            body,
        }
    }
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

/// Only the version of a frame, for a peer whose envelope differs from [`FrameHeader`] too.
#[derive(Deserialize)]
struct VersionProbe {
    v: u32,
}

/// Framing failed. Every variant ends the session, except a [`FrameError::Timeout`] from a
/// [`FrameReader`], which keeps what it received and may be called again.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("the pipe was closed")]
    Closed,
    /// `ErrorKind::UnexpectedEof` when the pipe closed in the middle of a frame (a truncated
    /// frame, as opposed to [`FrameError::Closed`] between frames).
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
///
/// Does not flush: on a pipe, `FlushFileBuffers` waits until the peer has read everything, which
/// could block the writer on a stalled peer. Pass an unbuffered writer (the pipe itself).
/// A body that serializes to more than [`MAX_FRAME_LEN`] bytes is [`FrameError::BadLength`] and
/// nothing is written.
pub fn write_frame<W: Write, T: Serialize>(
    writer: &mut W,
    frame: &Frame<T>,
) -> Result<(), FrameError> {
    let bytes = encode_frame(frame)?;
    writer
        .write_all(&bytes)
        .map_err(|error| match error.kind() {
            kind if is_closed(kind) => FrameError::Closed,
            kind if is_timeout(kind) => FrameError::Timeout,
            kind => FrameError::Io(kind),
        })
}

/// Reads one frame. Checks the length before allocating, then the version (from [`FrameHeader`],
/// before the body is parsed); the caller checks `seq`.
///
/// A [`FrameError::Timeout`] may leave part of a frame consumed, so it ends the session here; use
/// a [`FrameReader`] to poll with short timeouts.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<Frame<T>, FrameError> {
    let mut one_shot = FrameReader::new();
    let body = one_shot.read_body(reader)?;
    decode_body(&body)
}

/// Numbers the frames one side sends: 1, 2, 3, … (section E.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSequencer {
    next: u64,
}

impl FrameSequencer {
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    /// Wraps `body` in a frame of this build's version with the next sequence number.
    pub fn frame<T>(&mut self, body: T) -> Frame<T> {
        let seq = self.next;
        self.next = self.next.saturating_add(1);
        Frame::new(seq, body)
    }
}

impl Default for FrameSequencer {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads the frames of one direction: checks length, version and sequence (1, 2, 3, …) of each.
///
/// A read that times out ([`FrameError::Timeout`]) keeps the bytes of a partly received frame and
/// can be retried, so the helper can wait for a decision in short ticks without ever losing
/// synchronization. Any other error is final: every later call returns it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameReader {
    /// Bytes of the frame being received: the length prefix, then the body.
    partial: Vec<u8>,
    next_seq: u64,
    failed: Option<FrameError>,
}

impl FrameReader {
    /// A reader that expects sequence number 1 first.
    pub const fn new() -> Self {
        Self {
            partial: Vec::new(),
            next_seq: 1,
            failed: None,
        }
    }

    /// Reads the next frame, waiting as long as `reader` does.
    pub fn read<R: Read, T: DeserializeOwned>(
        &mut self,
        reader: &mut R,
    ) -> Result<Frame<T>, FrameError> {
        let result = self
            .read_body(reader)
            .and_then(|body| decode_body::<T>(&body))
            .and_then(|frame| {
                if frame.seq == self.next_seq {
                    self.next_seq = self.next_seq.saturating_add(1);
                    Ok(frame)
                } else {
                    Err(FrameError::Sequence {
                        found: frame.seq,
                        expected: self.next_seq,
                    })
                }
            });
        if let Err(error) = &result
            && *error != FrameError::Timeout
        {
            self.failed = Some(error.clone());
        }
        result
    }

    /// True while part of a frame has arrived and the rest has not.
    pub fn has_partial_frame(&self) -> bool {
        !self.partial.is_empty()
    }

    /// Receives one whole frame and returns its body (without the length prefix).
    fn read_body<R: Read>(&mut self, reader: &mut R) -> Result<Vec<u8>, FrameError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        self.fill(reader, LEN_PREFIX)?;
        let mut prefix = [0u8; LEN_PREFIX];
        prefix.copy_from_slice(&self.partial[..LEN_PREFIX]);
        let len = u32::from_le_bytes(prefix);
        if len == 0 || len > MAX_FRAME_LEN {
            return Err(FrameError::BadLength {
                len,
                max: MAX_FRAME_LEN,
            });
        }
        // `len` is at most MAX_FRAME_LEN, so the conversion and the allocation are small.
        let total = LEN_PREFIX + len as usize;
        self.fill(reader, total)?;
        let mut body = std::mem::take(&mut self.partial);
        body.drain(..LEN_PREFIX);
        Ok(body)
    }

    /// Reads until `partial` holds `target` bytes.
    fn fill<R: Read>(&mut self, reader: &mut R, target: usize) -> Result<(), FrameError> {
        while self.partial.len() < target {
            let start = self.partial.len();
            self.partial.resize(target, 0);
            let read = reader.read(&mut self.partial[start..]);
            let received = match &read {
                Ok(count) => (*count).min(target - start),
                Err(_) => 0,
            };
            self.partial.truncate(start + received);
            match read {
                Ok(0) => return Err(closed_at(start)),
                Ok(_) => {}
                Err(error) => match error.kind() {
                    ErrorKind::Interrupted => {}
                    kind if is_timeout(kind) => return Err(FrameError::Timeout),
                    kind if is_closed(kind) => return Err(closed_at(start)),
                    kind => return Err(FrameError::Io(kind)),
                },
            }
        }
        Ok(())
    }
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

/// The length prefix and the JSON of `frame`.
fn encode_frame<T: Serialize>(frame: &Frame<T>) -> Result<Vec<u8>, FrameError> {
    let json = serde_json::to_vec(frame).map_err(|error| FrameError::Json(error.to_string()))?;
    let len = u32::try_from(json.len()).unwrap_or(u32::MAX);
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(FrameError::BadLength {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    let mut bytes = Vec::with_capacity(LEN_PREFIX + json.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

/// Checks the version from the envelope alone, then parses the whole frame.
fn decode_body<T: DeserializeOwned>(body: &[u8]) -> Result<Frame<T>, FrameError> {
    let json_error = |error: serde_json::Error| FrameError::Json(error.to_string());
    let header: FrameHeader = match serde_json::from_slice(body) {
        Ok(header) => header,
        Err(error) => {
            // A peer of another version may shape its envelope differently: report the version
            // if there is one to report.
            return Err(match serde_json::from_slice::<VersionProbe>(body) {
                Ok(probe) if probe.v != PROTOCOL_VERSION => FrameError::Version {
                    found: probe.v,
                    expected: PROTOCOL_VERSION,
                },
                _ => json_error(error),
            });
        }
    };
    if header.v != PROTOCOL_VERSION {
        return Err(FrameError::Version {
            found: header.v,
            expected: PROTOCOL_VERSION,
        });
    }
    serde_json::from_slice(body).map_err(json_error)
}

/// `Closed` between frames, a truncated frame otherwise.
fn closed_at(received: usize) -> FrameError {
    if received == 0 {
        FrameError::Closed
    } else {
        FrameError::Io(ErrorKind::UnexpectedEof)
    }
}

/// Errors that mean the other end is gone (`ERROR_BROKEN_PIPE`, `ERROR_NO_DATA`, …).
fn is_closed(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotConnected
    )
}

fn is_timeout(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Body {
        text: String,
    }

    fn body(text: &str) -> Body {
        Body {
            text: text.to_string(),
        }
    }

    fn encoded(frame: &Frame<Body>) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, frame).unwrap();
        bytes
    }

    /// A length prefix and `json` as the body.
    fn raw(json: &str) -> Vec<u8> {
        let mut bytes = u32::try_from(json.len()).unwrap().to_le_bytes().to_vec();
        bytes.extend_from_slice(json.as_bytes());
        bytes
    }

    /// A reader that hands out scripted chunks and errors, one per `read` call.
    struct Scripted {
        steps: VecDeque<io::Result<Vec<u8>>>,
        pending: Vec<u8>,
    }

    impl Scripted {
        fn new(steps: impl IntoIterator<Item = io::Result<Vec<u8>>>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
                pending: Vec::new(),
            }
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pending.is_empty() {
                match self.steps.pop_front() {
                    None => return Ok(0),
                    Some(Err(error)) => return Err(error),
                    Some(Ok(chunk)) => self.pending = chunk,
                }
            }
            let count = buf.len().min(self.pending.len());
            buf[..count].copy_from_slice(&self.pending[..count]);
            self.pending.drain(..count);
            Ok(count)
        }
    }

    fn timeout() -> io::Result<Vec<u8>> {
        Err(io::Error::from(ErrorKind::TimedOut))
    }

    #[test]
    fn round_trip() {
        let frame = Frame::new(7, body("héllo \"quoted\" ✓"));
        assert_eq!(frame.v, PROTOCOL_VERSION);
        let bytes = encoded(&frame);
        let json = serde_json::to_vec(&frame).unwrap();
        assert_eq!(bytes[..4], u32::try_from(json.len()).unwrap().to_le_bytes());
        assert_eq!(bytes[4..], json);
        let read: Frame<Body> = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(read, frame);
    }

    #[test]
    fn several_frames_in_one_stream() {
        let mut sequencer = FrameSequencer::new();
        let frames: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|text| sequencer.frame(body(text)))
            .collect();
        assert_eq!(
            frames.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let stream: Vec<u8> = frames.iter().flat_map(encoded).collect();
        let mut stream = stream.as_slice();
        let mut reader = FrameReader::new();
        for frame in &frames {
            assert_eq!(&reader.read::<_, Body>(&mut stream).unwrap(), frame);
        }
        assert_eq!(reader.read::<_, Body>(&mut stream), Err(FrameError::Closed));
    }

    #[test]
    fn one_write_per_frame() {
        struct CountingWriter {
            writes: usize,
            bytes: Vec<u8>,
        }
        impl Write for CountingWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                self.bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("write_frame must not flush");
            }
        }
        let mut writer = CountingWriter {
            writes: 0,
            bytes: Vec::new(),
        };
        let frame = Frame::new(1, body(&"x".repeat(100_000)));
        write_frame(&mut writer, &frame).unwrap();
        assert_eq!(writer.writes, 1);
        assert_eq!(writer.bytes, encoded(&frame));
    }

    #[test]
    fn zero_length_is_rejected() {
        let result = read_frame::<_, Body>(&mut [0u8, 0, 0, 0].as_slice());
        assert_eq!(
            result,
            Err(FrameError::BadLength {
                len: 0,
                max: MAX_FRAME_LEN
            })
        );
    }

    #[test]
    fn oversized_length_is_rejected_before_reading_the_body() {
        for len in [MAX_FRAME_LEN + 1, u32::MAX] {
            // Only the prefix is there: reading on would end in a truncation error instead.
            let prefix = len.to_le_bytes();
            let result = read_frame::<_, Body>(&mut prefix.as_slice());
            assert_eq!(
                result,
                Err(FrameError::BadLength {
                    len,
                    max: MAX_FRAME_LEN
                })
            );
        }
    }

    #[test]
    fn the_largest_frame_is_accepted() {
        let envelope = format!(r#"{{"v":{PROTOCOL_VERSION},"seq":1,"body":{{"text":""}}}}"#);
        let padding = MAX_FRAME_LEN as usize - envelope.len();
        let json = format!(
            r#"{{"v":{PROTOCOL_VERSION},"seq":1,"body":{{"text":"{}"}}}}"#,
            "x".repeat(padding)
        );
        assert_eq!(json.len(), MAX_FRAME_LEN as usize);
        let frame: Frame<Body> = read_frame(&mut raw(&json).as_slice()).unwrap();
        assert_eq!(frame.body.text.len(), padding);
        // And write_frame produces it.
        assert_eq!(encoded(&frame), raw(&json));
    }

    #[test]
    fn write_frame_refuses_oversized_bodies() {
        let frame = Frame::new(1, body(&"x".repeat(MAX_FRAME_LEN as usize)));
        let mut bytes = Vec::new();
        let result = write_frame(&mut bytes, &frame);
        assert!(
            matches!(result, Err(FrameError::BadLength { len, max: MAX_FRAME_LEN }) if len > MAX_FRAME_LEN),
            "{result:?}"
        );
        assert!(bytes.is_empty());
    }

    #[test]
    fn truncated_frames() {
        let bytes = encoded(&Frame::new(1, body("truncated")));
        // In the length prefix, and in the body.
        for cut in [1, 3, 4, 5, bytes.len() - 1] {
            let result = read_frame::<_, Body>(&mut &bytes[..cut]);
            assert_eq!(
                result,
                Err(FrameError::Io(ErrorKind::UnexpectedEof)),
                "{cut}"
            );
        }
        // Nothing at all: the peer closed between frames.
        assert_eq!(
            read_frame::<_, Body>(&mut [].as_slice()),
            Err(FrameError::Closed)
        );
    }

    #[test]
    fn corrupt_json() {
        let current = PROTOCOL_VERSION;
        for json in [
            "{".to_string(),
            "null".to_string(),
            "[1,2]".to_string(),
            "\"text\"".to_string(),
            r#"{"seq":1,"body":{"text":"x"}}"#.to_string(),
            format!(r#"{{"v":{current},"body":{{"text":"x"}}}}"#),
            format!(r#"{{"v":{current},"seq":1}}"#),
            format!(r#"{{"v":{current},"seq":1,"body":{{"text":1}}}}"#),
            format!(r#"{{"v":{current},"seq":1,"body":{{"text":"x"}}}} trailing"#),
            format!(r#"{{"v":{current},"seq":1,"body":{{"text":"x"}},"extra":0}}"#),
            format!(r#"{{"v":"{current}","seq":1,"body":{{"text":"x"}}}}"#),
            format!(r#"{{"v":{current},"seq":-1,"body":{{"text":"x"}}}}"#),
        ] {
            let result = read_frame::<_, Body>(&mut raw(&json).as_slice());
            assert!(
                matches!(result, Err(FrameError::Json(_))),
                "{json} → {result:?}"
            );
        }
        // Invalid UTF-8 inside a string.
        let mut bytes = format!(r#"{{"v":{current},"seq":1,"body":{{"text":"#).into_bytes();
        bytes.extend_from_slice(&[b'"', 0xff, 0xfe, b'"', b'}', b'}']);
        let mut framed = u32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec();
        framed.extend_from_slice(&bytes);
        let result = read_frame::<_, Body>(&mut framed.as_slice());
        assert!(matches!(result, Err(FrameError::Json(_))), "{result:?}");
    }

    /// Design review S11: a peer of another version is reported as such, even when its body (or
    /// its envelope) does not parse as this version's message.
    #[test]
    fn version_skew_is_version_not_json() {
        let other = PROTOCOL_VERSION + 1;
        let cases = [
            // A body this version does not know.
            (
                format!(r#"{{"v":{other},"seq":1,"body":{{"type":"brand-new","data":[1,2,3]}}}}"#),
                other,
            ),
            // Same body shape, other version.
            (
                format!(r#"{{"v":{other},"seq":1,"body":{{"text":"x"}}}}"#),
                other,
            ),
            // An envelope that changed too.
            (format!(r#"{{"v":{other},"id":"a","payload":{{}}}}"#), other),
            (format!(r#"{{"seq":"one","v":{other}}}"#), other),
            // Version 0.
            (r#"{"v":0,"seq":1,"body":null}"#.to_string(), 0),
        ];
        for (json, found) in cases {
            let result = read_frame::<_, Body>(&mut raw(&json).as_slice());
            assert_eq!(
                result,
                Err(FrameError::Version {
                    found,
                    expected: PROTOCOL_VERSION
                }),
                "{json}"
            );
        }
        // The header alone parses whatever the body is.
        let header: FrameHeader =
            serde_json::from_str(r#"{"v":9,"seq":4,"body":{"anything":[true]}}"#).unwrap();
        assert_eq!(header, FrameHeader { v: 9, seq: 4 });
    }

    #[test]
    fn io_errors() {
        let cases = [
            (ErrorKind::BrokenPipe, FrameError::Closed),
            (ErrorKind::ConnectionReset, FrameError::Closed),
            (ErrorKind::TimedOut, FrameError::Timeout),
            (ErrorKind::WouldBlock, FrameError::Timeout),
            (
                ErrorKind::PermissionDenied,
                FrameError::Io(ErrorKind::PermissionDenied),
            ),
        ];
        for (kind, expected) in cases {
            let mut reader = Scripted::new([Err(io::Error::from(kind))]);
            assert_eq!(
                read_frame::<_, Body>(&mut reader),
                Err(expected.clone()),
                "{kind:?}"
            );
        }
        // A broken pipe in the middle of a frame is a truncated frame.
        let bytes = encoded(&Frame::new(1, body("x")));
        let mut reader = Scripted::new([
            Ok(bytes[..6].to_vec()),
            Err(io::Error::from(ErrorKind::BrokenPipe)),
        ]);
        assert_eq!(
            read_frame::<_, Body>(&mut reader),
            Err(FrameError::Io(ErrorKind::UnexpectedEof))
        );
        // Interrupted reads are retried.
        let mut reader = Scripted::new([
            Err(io::Error::from(ErrorKind::Interrupted)),
            Ok(bytes.clone()),
        ]);
        assert_eq!(read_frame::<_, Body>(&mut reader).unwrap().body, body("x"));
        // Write errors.
        struct Failing(ErrorKind);
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(self.0))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let frame = Frame::new(1, body("x"));
        assert_eq!(
            write_frame(&mut Failing(ErrorKind::BrokenPipe), &frame),
            Err(FrameError::Closed)
        );
        assert_eq!(
            write_frame(&mut Failing(ErrorKind::TimedOut), &frame),
            Err(FrameError::Timeout)
        );
        assert_eq!(
            write_frame(&mut Failing(ErrorKind::Other), &frame),
            Err(FrameError::Io(ErrorKind::Other))
        );
    }

    #[test]
    fn reader_checks_the_sequence() {
        let frames = |seqs: &[u64]| -> Vec<u8> {
            seqs.iter()
                .flat_map(|&seq| encoded(&Frame::new(seq, body("x"))))
                .collect()
        };
        // Starts at 1.
        for first in [0, 2, u64::MAX] {
            let stream = frames(&[first]);
            assert_eq!(
                FrameReader::new().read::<_, Body>(&mut stream.as_slice()),
                Err(FrameError::Sequence {
                    found: first,
                    expected: 1
                })
            );
        }
        // Gap.
        let stream = frames(&[1, 2, 4]);
        let mut stream = stream.as_slice();
        let mut reader = FrameReader::new();
        reader.read::<_, Body>(&mut stream).unwrap();
        reader.read::<_, Body>(&mut stream).unwrap();
        let gap = Err(FrameError::Sequence {
            found: 4,
            expected: 3,
        });
        assert_eq!(reader.read::<_, Body>(&mut stream), gap);
        // The error is final.
        let more = frames(&[3]);
        assert_eq!(reader.read::<_, Body>(&mut more.as_slice()), gap);
        // Repeat.
        let stream = frames(&[1, 1]);
        let mut stream = stream.as_slice();
        let mut reader = FrameReader::new();
        reader.read::<_, Body>(&mut stream).unwrap();
        assert_eq!(
            reader.read::<_, Body>(&mut stream),
            Err(FrameError::Sequence {
                found: 1,
                expected: 2
            })
        );
    }

    #[test]
    fn reader_survives_timeouts_mid_frame() {
        let first = encoded(&Frame::new(1, body("first")));
        let second = encoded(&Frame::new(2, body("second")));
        let mut reader_input = Scripted::new([
            timeout(),
            Ok(first[..2].to_vec()),
            timeout(),
            Ok(first[2..9].to_vec()),
            timeout(),
            Ok(first[9..].iter().chain(&second[..3]).copied().collect()),
            timeout(),
            Ok(second[3..].to_vec()),
        ]);
        let mut reader = FrameReader::new();
        let mut received = Vec::new();
        let mut timeouts = 0;
        loop {
            match reader.read::<_, Body>(&mut reader_input) {
                Ok(frame) => received.push(frame.body.text),
                Err(FrameError::Timeout) => timeouts += 1,
                Err(FrameError::Closed) => break,
                Err(error) => panic!("{error:?}"),
            }
        }
        assert_eq!(received, ["first", "second"]);
        assert_eq!(timeouts, 4);
        assert!(!reader.has_partial_frame());
    }

    #[test]
    fn reader_errors_are_final() {
        let mut reader = FrameReader::new();
        let bad = [0u8, 0, 0, 0];
        let error = reader.read::<_, Body>(&mut bad.as_slice()).unwrap_err();
        assert_eq!(
            error,
            FrameError::BadLength {
                len: 0,
                max: MAX_FRAME_LEN
            }
        );
        let good = encoded(&Frame::new(1, body("x")));
        assert_eq!(reader.read::<_, Body>(&mut good.as_slice()), Err(error));
    }
}
