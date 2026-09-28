//! The update messages of protocol 3 (design m5b D.3, H.2): the caller reports its newest verified
//! manifest (`RecordTrust`), asks the helper to stage an update (`StageUpdate`) and sends the
//! installer bytes (`InstallerChunk`); the helper answers with [`UpdateMessage`].

use serde::{Deserialize, Serialize};

pub use mklm_update::UpdateRefusal;
pub use mklm_update::stage::CHUNK_LEN;

/// The client's newest verified manifest and the signature that verified it (design m5b C.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustReport {
    pub manifest: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageUpdateRequest {
    /// latest.json exactly as downloaded.
    pub manifest: String,
    /// The signature file (main or alternate) that verified, exactly as downloaded.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallerChunk {
    pub offset: u64,
    /// Lower-case hex of 1..=CHUNK_LEN bytes.
    pub hex: String,
}

impl InstallerChunk {
    pub fn new(offset: u64, bytes: &[u8]) -> InstallerChunk {
        InstallerChunk {
            offset,
            hex: encode_hex(bytes),
        }
    }

    /// `ChunkMalformed` for upper case, odd length, other characters, 0 or > CHUNK_LEN bytes.
    pub fn decode(&self) -> Result<Vec<u8>, UpdateRefusal> {
        // The length first: nothing is decoded of an oversized chunk.
        if self.hex.is_empty() || self.hex.len() > CHUNK_LEN * 2 {
            return Err(UpdateRefusal::ChunkMalformed);
        }
        decode_hex(&self.hex).ok_or(UpdateRefusal::ChunkMalformed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum UpdateMessage {
    /// Answer to `RecordTrust`; the session goes on either way.
    TrustRecorded {
        changed: bool,
    },
    TrustNotRecorded(UpdateRefusal),
    SendInstaller {
        name: String,
        size: u64,
        sha256: String,
        chunk_len: u32,
    },
    Received {
        bytes: u64,
    },
    /// H2 was started; waiting for it to be ready (up to `READY_WAIT`; RELIABILITY-5).
    StartingRunner,
    HandedOff {
        run_id: String,
        to_version: String,
    },
    Refused(UpdateRefusal),
}

/// Lower-case hex.
pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Lower-case hex only, even length; `None` otherwise.
pub fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|&[high, low]| Some((lower_hex_value(high)? << 4) | lower_hex_value(low)?))
        .collect()
}

/// `[0-9a-f]` → its value; upper case is refused (one spelling).
fn lower_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_in_lower_case() {
        let bytes: Vec<u8> = (0..=255).collect();
        let text = encode_hex(&bytes);
        assert_eq!(text.len(), 512);
        assert!(text.starts_with("000102") && text.ends_with("fdfeff"));
        assert_eq!(decode_hex(&text), Some(bytes));
        assert_eq!(encode_hex(&[]), "");
        assert_eq!(decode_hex(""), Some(Vec::new()));
        assert_eq!(encode_hex(&[0x4d, 0x5a, 0x90, 0x00]), "4d5a9000");
    }

    #[test]
    fn hex_has_one_spelling() {
        for text in [
            "A0", "0A", "abc", "0", "0g", "g0", " 00", "00 ", "0x00", "-1", "é0", "\u{0}0",
        ] {
            assert_eq!(decode_hex(text), None, "{text:?}");
        }
    }

    #[test]
    fn installer_chunks() {
        let chunk = InstallerChunk::new(65_536, &[0x4d, 0x5a]);
        assert_eq!(chunk.offset, 65_536);
        assert_eq!(chunk.hex, "4d5a");
        assert_eq!(chunk.decode(), Ok(vec![0x4d, 0x5a]));

        let full = InstallerChunk::new(0, &vec![0xab; CHUNK_LEN]);
        assert_eq!(full.decode().map(|bytes| bytes.len()), Ok(CHUNK_LEN));

        let refused = [
            String::new(),
            "4D5A".to_string(),
            "4d5".to_string(),
            "4d5z".to_string(),
            "4d 5a".to_string(),
            "ab".repeat(CHUNK_LEN + 1),
        ];
        for hex in refused {
            let chunk = InstallerChunk { offset: 0, hex };
            assert_eq!(
                chunk.decode(),
                Err(UpdateRefusal::ChunkMalformed),
                "{}",
                &chunk.hex[..chunk.hex.len().min(16)]
            );
        }
        assert_eq!(CHUNK_LEN, 64 * 1024);
    }
}
