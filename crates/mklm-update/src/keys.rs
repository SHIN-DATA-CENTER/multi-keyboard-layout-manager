//! Key IDs, fingerprints, the trust anchors file and the keys a manifest may be signed with
//! (design m5b B.2, C.2).
//!
//! Every key is read twice: by `minisign_verify::PublicKey::from_base64` (which verifies with it),
//! and by this module's strict base64 decoder, which takes the key ID (bytes 2..10) and the
//! fingerprint (SHA-256 of all 42 bytes) from the same bytes. A key whose text is not canonical
//! base64, or whose algorithm is not `Ed`, is not a key.

use std::collections::BTreeSet;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::base64;

/// A minisign key ID. Text form: 16 upper-case hex digits of the 8 bytes read as a little-endian
/// u64 (design m5b B.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    /// Exactly 16 upper-case hex digits.
    pub fn parse(text: &str) -> Result<KeyId, KeyError> {
        let bad = || KeyError::BadKeyId {
            text: text.to_string(),
        };
        let digits = text.as_bytes();
        if digits.len() != 16
            || !digits
                .iter()
                .all(|&b| matches!(b, b'0'..=b'9' | b'A'..=b'F'))
        {
            return Err(bad());
        }
        let value = u64::from_str_radix(text, 16).map_err(|_| bad())?;
        Ok(KeyId(value.to_le_bytes()))
    }

    pub fn to_text(self) -> String {
        format!("{:016X}", u64::from_le_bytes(self.0))
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

/// SHA-256 of the decoded public key (42 bytes: algorithm, key ID, Ed25519 key). Scopes recorded
/// revocations (SECURITY-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyFingerprint(pub [u8; 32]);

impl KeyFingerprint {
    /// 64 lower-case hex digits.
    pub fn to_hex(&self) -> String {
        crate::manifest::lower_hex(&self.0)
    }

    pub fn parse_hex(text: &str) -> Option<KeyFingerprint> {
        crate::manifest::parse_lower_hex_32(text).map(KeyFingerprint)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRole {
    Primary,
    Backup,
}

impl KeyRole {
    /// The word of the trust anchors file: `primary` / `backup`.
    pub fn as_str(self) -> &'static str {
        match self {
            KeyRole::Primary => "primary",
            KeyRole::Backup => "backup",
        }
    }
}

/// One `primary` / `backup` line of the trust anchors file (design m5b B.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorEntry {
    pub role: KeyRole,
    pub id: KeyId,
    /// The base64 line of the minisign public key file.
    pub public_key: String,
}

/// The parsed trust anchors file (OPS-UX-TEST-1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnchorsFile {
    pub keys: Vec<AnchorEntry>,
    pub revoked: Vec<KeyId>,
}

/// Path of the file in the repository, for `git show <tag>:<path>` (xtask).
pub const ANCHORS_REPO_PATH: &str = "crates/mklm-update/trust/anchors.txt";
/// The file of this build.
pub const ANCHORS_TEXT: &str = include_str!("../trust/anchors.txt");

/// Strict: comments (`#`) and blank lines skipped; `<role> <KEYID> <base64>` or `revoked <KEYID>`;
/// anything else is `BadAnchorsLine { line }` (1-based). Does not decode the keys.
///
/// Lines end with LF or CRLF (a Windows checkout). A comment line starts with `#` in its first
/// column; a blank line is empty or holds spaces and tabs only. Items are separated by one or more
/// spaces or tabs; a line may not start or end with them. The role is lower case; the key ID is
/// [`KeyId::parse`]'s form; the key is a run of base64 characters (`A-Z a-z 0-9 + / =`), checked
/// no further here. Anything else, a byte order mark included, is malformed.
pub fn parse_anchors(text: &str) -> Result<AnchorsFile, KeyError> {
    let mut file = AnchorsFile::default();
    for (index, line) in text.lines().enumerate() {
        let bad = || KeyError::BadAnchorsLine { line: index + 1 };
        let is_gap = |c: char| c == ' ' || c == '\t';
        if line.starts_with('#') || line.chars().all(is_gap) {
            continue;
        }
        if line.starts_with(is_gap) || line.ends_with(is_gap) {
            return Err(bad());
        }
        let fields: Vec<&str> = line.split(is_gap).filter(|f| !f.is_empty()).collect();
        let key_id = |text: &str| KeyId::parse(text).map_err(|_| bad());
        match fields.as_slice() {
            [role @ ("primary" | "backup"), id, public_key] => {
                if !is_base64_text(public_key) {
                    return Err(bad());
                }
                file.keys.push(AnchorEntry {
                    role: if *role == "primary" {
                        KeyRole::Primary
                    } else {
                        KeyRole::Backup
                    },
                    id: key_id(id)?,
                    public_key: (*public_key).to_string(),
                });
            }
            ["revoked", id] => file.revoked.push(key_id(id)?),
            _ => return Err(bad()),
        }
    }
    Ok(file)
}

/// Only characters of the standard base64 alphabet and padding.
fn is_base64_text(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// A minisign public key read strictly: `Ed`, the key ID, the Ed25519 key.
struct DecodedKey {
    id: KeyId,
    fingerprint: KeyFingerprint,
    verifier: minisign_verify::PublicKey,
}

fn decode_public_key(public_key_base64: &str) -> Option<DecodedKey> {
    let bytes = base64::decode(public_key_base64)?;
    if bytes.len() != 42 || &bytes[..2] != b"Ed" {
        return None;
    }
    let verifier = minisign_verify::PublicKey::from_base64(public_key_base64).ok()?;
    Some(DecodedKey {
        id: KeyId(bytes[2..10].try_into().ok()?),
        fingerprint: KeyFingerprint(Sha256::digest(&bytes).into()),
        verifier,
    })
}

/// One key of a [`TrustAnchors`].
#[derive(Clone)]
struct AnchorKey {
    id: KeyId,
    /// `None` for the development key (development builds only).
    role: Option<KeyRole>,
    fingerprint: KeyFingerprint,
    public_key: minisign_verify::PublicKey,
}

/// The parsed keys a manifest may be signed with. `Debug` prints IDs and roles only.
#[derive(Clone)]
pub struct TrustAnchors {
    keys: Vec<AnchorKey>,
    /// The `revoked` lines of the file (sorted, without duplicates).
    revoked: Vec<KeyId>,
}

impl fmt::Debug for TrustAnchors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrustAnchors")
            .field(
                "keys",
                &self
                    .keys
                    .iter()
                    .map(|key| (key.id.to_text(), key.role))
                    .collect::<Vec<_>>(),
            )
            .field(
                "revoked",
                &self
                    .revoked
                    .iter()
                    .map(|id| id.to_text())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl TrustAnchors {
    /// `ANCHORS_TEXT` only, in every build profile (xtask's production commands, tests of the
    /// committed file). `NotConfigured` when the file has no key (OPS-UX-TEST-7).
    pub fn release() -> Result<TrustAnchors, KeyError> {
        Self::from_file(&parse_anchors(ANCHORS_TEXT)?)
    }

    /// What the GUI, the CLI and the helper verify with: `release()`; in development builds
    /// (`cfg(all(debug_assertions, mklm_update_dev))`) plus `MKLM_UPDATE_DEV_PUBKEY` as the
    /// development key (design m5b A.10).
    pub fn for_this_build() -> Result<TrustAnchors, KeyError> {
        // The development branch references DEV_MARKER, so that the positive control finds it in
        // every development executable (design m5b A.10; FIX-VERIFICATION-3).
        #[cfg(all(debug_assertions, mklm_update_dev))]
        if let Some(development_key) = crate::dev::extra_key() {
            // A development build may have no release key at all (design m5b C.2).
            let file = parse_anchors(ANCHORS_TEXT)?;
            let anchors = Self::build(
                file.keys
                    .iter()
                    .map(|entry| (Some(entry.role), Some(entry.id), entry.public_key.as_str())),
                &file.revoked,
                false,
            )?;
            return anchors.with_dev_key(development_key);
        }
        Self::release()
    }

    /// A file read from another tag (xtask); the same checks.
    pub fn from_file(file: &AnchorsFile) -> Result<TrustAnchors, KeyError> {
        Self::build(
            file.keys
                .iter()
                .map(|entry| (Some(entry.role), Some(entry.id), entry.public_key.as_str())),
            &file.revoked,
            true,
        )
    }

    /// Tests: the same checks.
    pub fn from_keys(keys: &[(KeyRole, &str)], revoked: &[&str]) -> Result<TrustAnchors, KeyError> {
        let revoked = revoked
            .iter()
            .map(|text| KeyId::parse(text))
            .collect::<Result<Vec<_>, _>>()?;
        Self::build(
            keys.iter().map(|&(role, key)| (Some(role), None, key)),
            &revoked,
            true,
        )
    }

    /// Adds the development key (no role; accepted only with `DEV_TRUSTED_COMMENT_PREFIX`,
    /// records nothing). Development builds only; `for_this_build` and the development tests use
    /// it (design m5b A.10, F.1).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn with_dev_key(mut self, public_key_base64: &str) -> Result<TrustAnchors, KeyError> {
        let index = self.keys.len();
        let decoded =
            decode_public_key(public_key_base64).ok_or(KeyError::BadPublicKey { index })?;
        if self.keys.iter().any(|key| key.id == decoded.id) {
            return Err(KeyError::DuplicateId);
        }
        self.keys.push(AnchorKey {
            id: decoded.id,
            role: None,
            fingerprint: decoded.fingerprint,
            public_key: decoded.verifier,
        });
        Ok(self)
    }

    /// Decodes and checks every key (index = position in `keys`, from 0): `BadPublicKey`,
    /// `IdMismatch` against the stated ID, `DuplicateId`; `NotConfigured` without any key when
    /// `need_key`.
    fn build<'k>(
        keys: impl Iterator<Item = (Option<KeyRole>, Option<KeyId>, &'k str)>,
        revoked: &[KeyId],
        need_key: bool,
    ) -> Result<TrustAnchors, KeyError> {
        let mut anchors = TrustAnchors {
            keys: Vec::new(),
            revoked: revoked
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        };
        for (index, (role, stated, text)) in keys.enumerate() {
            let decoded = decode_public_key(text).ok_or(KeyError::BadPublicKey { index })?;
            if stated.is_some_and(|stated| stated != decoded.id) {
                return Err(KeyError::IdMismatch { index });
            }
            if anchors.keys.iter().any(|key| key.id == decoded.id) {
                return Err(KeyError::DuplicateId);
            }
            anchors.keys.push(AnchorKey {
                id: decoded.id,
                role,
                fingerprint: decoded.fingerprint,
                public_key: decoded.verifier,
            });
        }
        if need_key && anchors.keys.is_empty() {
            return Err(KeyError::NotConfigured);
        }
        Ok(anchors)
    }

    pub fn ids(&self) -> Vec<(KeyId, KeyRole)> {
        self.keys
            .iter()
            .filter_map(|key| key.role.map(|role| (key.id, role)))
            .collect()
    }

    pub fn role_of(&self, id: KeyId) -> Option<KeyRole> {
        self.key(id).and_then(|key| key.role)
    }

    pub fn fingerprint_of(&self, id: KeyId) -> Option<KeyFingerprint> {
        self.key(id).map(|key| key.fingerprint)
    }

    /// A `revoked` line of the file.
    pub fn revoked_by_build(&self, id: KeyId) -> bool {
        self.revoked.contains(&id)
    }

    pub fn revoked_ids(&self) -> Vec<KeyId> {
        self.revoked.clone()
    }

    /// Always false outside development builds.
    pub fn is_dev_key(&self, id: KeyId) -> bool {
        cfg!(all(debug_assertions, mklm_update_dev))
            && self.key(id).is_some_and(|key| key.role.is_none())
    }

    /// `xtask check-keys`: exactly one primary and one backup key, none revoked. `Roles`.
    pub fn check_release_roles(&self) -> Result<(), KeyError> {
        let count = |role| {
            self.keys
                .iter()
                .filter(|key| key.role == Some(role))
                .count()
        };
        let revoked = self
            .keys
            .iter()
            .any(|key| key.role.is_some() && self.revoked_by_build(key.id));
        if count(KeyRole::Primary) != 1 || count(KeyRole::Backup) != 1 || revoked {
            return Err(KeyError::Roles);
        }
        Ok(())
    }

    /// The verifier of a known key.
    pub(crate) fn verifier(&self, id: KeyId) -> Option<&minisign_verify::PublicKey> {
        self.key(id).map(|key| &key.public_key)
    }

    fn key(&self, id: KeyId) -> Option<&AnchorKey> {
        self.keys.iter().find(|key| key.id == id)
    }
}

/// The key ID inside a minisign public key (base64 line), strict base64.
pub fn public_key_id(public_key_base64: &str) -> Result<KeyId, KeyError> {
    decode_public_key(public_key_base64)
        .map(|key| key.id)
        .ok_or(KeyError::BadPublicKey { index: 0 })
}

pub fn public_key_fingerprint(public_key_base64: &str) -> Result<KeyFingerprint, KeyError> {
    decode_public_key(public_key_base64)
        .map(|key| key.fingerprint)
        .ok_or(KeyError::BadPublicKey { index: 0 })
}

/// The key ID inside a minisign signature file (its second line), strict base64.
pub fn signature_key_id(signature_text: &str) -> Result<KeyId, KeyError> {
    parse_signature_text(signature_text)
        .map(|parsed| parsed.key_id)
        .ok_or(KeyError::BadSignatureText)
}

/// The parts of a minisign signature file this crate reads itself (design m5b A.4, C.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignatureText<'a> {
    /// `ED` (prehashed) or `Ed` (legacy).
    pub(crate) algorithm: [u8; 2],
    pub(crate) key_id: KeyId,
    /// After `trusted comment: `.
    pub(crate) trusted_comment: &'a str,
}

impl SignatureText<'_> {
    pub(crate) fn is_prehashed(&self) -> bool {
        &self.algorithm == b"ED"
    }
}

/// Strict form of a minisign signature file: exactly four lines (LF or CRLF, the last line break
/// optional); the second a canonical base64 of 74 bytes (algorithm `ED` or `Ed`, key ID, Ed25519
/// signature); the third `trusted comment: …`; the fourth a canonical base64 of 64 bytes. The
/// first line (the untrusted comment) is not read.
pub(crate) fn parse_signature_text(text: &str) -> Option<SignatureText<'_>> {
    let body = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let lines: Vec<&str> = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let [_untrusted, signature, trusted, global] = lines.as_slice() else {
        return None;
    };
    if lines.iter().any(|line| line.contains('\r')) {
        return None;
    }
    let signature = base64::decode(signature)?;
    if signature.len() != 74 || !matches!(&signature[..2], b"ED" | b"Ed") {
        return None;
    }
    if base64::decode(global)?.len() != 64 {
        return None;
    }
    let trusted_comment = trusted.strip_prefix("trusted comment: ")?;
    Some(SignatureText {
        algorithm: [signature[0], signature[1]],
        key_id: KeyId(signature[2..10].try_into().ok()?),
        trusted_comment,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("this build has no update keys")]
    NotConfigured,
    #[error("line {line} of the trust anchors file is malformed")]
    BadAnchorsLine { line: usize },
    #[error("key {index} is not a minisign public key")]
    BadPublicKey { index: usize },
    #[error("{text:?} is not a key ID")]
    BadKeyId { text: String },
    #[error("key {index}: its ID does not match the public key")]
    IdMismatch { index: usize },
    #[error("two keys have the same ID")]
    DuplicateId,
    #[error("the keys must be exactly one primary and one backup key, none of them revoked")]
    Roles,
    #[error("the signature file is malformed")]
    BadSignatureText,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// The minisign author's public key (a real minisign key, used here only as well-formed data).
    const PRIMARY_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    /// Its key ID as minisign prints it.
    const PRIMARY_KEY_ID: &str = "E7620F1842B4E81F";

    fn keypair() -> minisign::KeyPair {
        minisign::KeyPair::generate_unencrypted_keypair().expect("a throwaway key pair")
    }

    fn key_text(pair: &minisign::KeyPair) -> String {
        pair.pk.to_base64()
    }

    fn id_of(pair: &minisign::KeyPair) -> KeyId {
        KeyId(pair.pk.keynum().try_into().unwrap())
    }

    #[test]
    fn key_ids_are_little_endian_upper_hex() {
        let id = KeyId::parse("8F1A2B3C4D5E6F70").unwrap();
        assert_eq!(id.0, [0x70, 0x6F, 0x5E, 0x4D, 0x3C, 0x2B, 0x1A, 0x8F]);
        assert_eq!(id.to_text(), "8F1A2B3C4D5E6F70");
        assert_eq!(id.to_string(), "8F1A2B3C4D5E6F70");
        assert_eq!(
            KeyId([1, 0, 0, 0, 0, 0, 0, 0]).to_text(),
            "0000000000000001"
        );
        for text in ["0000000000000000", "FFFFFFFFFFFFFFFF", "0123456789ABCDEF"] {
            assert_eq!(KeyId::parse(text).unwrap().to_text(), text);
        }
        assert_eq!(
            public_key_id(PRIMARY_KEY).unwrap().to_text(),
            PRIMARY_KEY_ID
        );
    }

    #[test]
    fn key_ids_have_one_spelling() {
        for text in [
            "",
            "8f1a2b3c4d5e6f70",
            "8F1A2B3C4D5E6F7",
            "8F1A2B3C4D5E6F700",
            "8F1A2B3C4D5E6F7G",
            " 8F1A2B3C4D5E6F70",
            "8F1A2B3C4D5E6F70 ",
            "+F1A2B3C4D5E6F70",
            "0x8F1A2B3C4D5E6F",
            "８F1A2B3C4D5E6F7",
        ] {
            assert_eq!(
                KeyId::parse(text),
                Err(KeyError::BadKeyId {
                    text: text.to_string()
                }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn fingerprints_are_the_sha256_of_the_decoded_key() {
        let pair = keypair();
        let fingerprint = public_key_fingerprint(&key_text(&pair)).unwrap();
        let expected: [u8; 32] = Sha256::digest(pair.pk.to_bytes()).into();
        assert_eq!(fingerprint, KeyFingerprint(expected));
        let hex = fingerprint.to_hex();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(KeyFingerprint::parse_hex(&hex), Some(fingerprint));
        assert_eq!(KeyFingerprint::parse_hex(&hex.to_uppercase()), None);
        assert_eq!(KeyFingerprint::parse_hex(&hex[1..]), None);
        assert_eq!(KeyFingerprint::parse_hex(&format!("{hex}0")), None);
        assert_eq!(public_key_id(&key_text(&pair)), Ok(id_of(&pair)));
    }

    #[test]
    fn public_keys_are_read_strictly() {
        let pair = keypair();
        let text = key_text(&pair);
        let bytes = pair.pk.to_bytes();
        // Another algorithm, a secret key's length, a signature in place of a key.
        let mut other_algorithm = bytes.clone();
        other_algorithm[1] = b'D';
        for bad in [
            String::new(),
            text[..text.len() - 4].to_string(),
            format!("{text}AAAA"),
            format!(" {text}"),
            text.replace('+', "-"),
            encode(&other_algorithm),
            encode(&bytes[..41]),
        ] {
            if bad == text {
                continue;
            }
            assert_eq!(
                public_key_id(&bad),
                Err(KeyError::BadPublicKey { index: 0 }),
                "{bad:?}"
            );
        }
    }

    /// Standard padded base64, for building test data.
    fn encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let word = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((word >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// The committed file: comments only until the maintainer adds the keys (design m5b B.5), then
    /// exactly what `xtask check-keys` demands.
    #[test]
    fn the_committed_file() {
        assert!(ANCHORS_TEXT.starts_with("# MKLM update trust anchors (design m5b B.2)."));
        assert_eq!(ANCHORS_REPO_PATH, "crates/mklm-update/trust/anchors.txt");
        let file = parse_anchors(ANCHORS_TEXT).unwrap();
        if file.keys.is_empty() {
            // No key: every build of this tree is `NotConfigured` (design m5b B.2).
            assert_eq!(
                TrustAnchors::release().map(|_| ()),
                Err(KeyError::NotConfigured)
            );
        } else {
            let anchors = TrustAnchors::release().unwrap();
            assert_eq!(anchors.check_release_roles(), Ok(()));
        }
    }

    #[test]
    fn parses_keys_revocations_comments_and_blank_lines() {
        let text = format!(
            "# comment\r\n\
             primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY}\r\n\
             \r\n\
             backup  0123456789ABCDEF\t{PRIMARY_KEY}\n\
             \t \n\
             #revoked 0000000000000000\n\
             revoked 1111222233334444\n\
             revoked\t\t2222333344445555"
        );
        let file = parse_anchors(&text).unwrap();
        assert_eq!(
            file,
            AnchorsFile {
                keys: vec![
                    AnchorEntry {
                        role: KeyRole::Primary,
                        id: KeyId::parse("8F1A2B3C4D5E6F70").unwrap(),
                        public_key: PRIMARY_KEY.to_string(),
                    },
                    AnchorEntry {
                        role: KeyRole::Backup,
                        id: KeyId::parse("0123456789ABCDEF").unwrap(),
                        public_key: PRIMARY_KEY.to_string(),
                    },
                ],
                revoked: vec![
                    KeyId::parse("1111222233334444").unwrap(),
                    KeyId::parse("2222333344445555").unwrap(),
                ],
            }
        );
        assert_eq!(parse_anchors(""), Ok(AnchorsFile::default()));
        assert_eq!(parse_anchors("\n\n# only\n"), Ok(AnchorsFile::default()));
    }

    #[test]
    fn malformed_lines_are_refused_with_their_number() {
        let good = format!("primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY}");
        let bad_lines = [
            format!("Primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY}"),
            format!("main 8F1A2B3C4D5E6F70 {PRIMARY_KEY}"),
            format!("primary 8f1a2b3c4d5e6f70 {PRIMARY_KEY}"),
            format!("primary 8F1A2B3C4D5E6F7 {PRIMARY_KEY}"),
            "primary 8F1A2B3C4D5E6F70".to_string(),
            format!("primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY} extra"),
            format!("primary {PRIMARY_KEY}"),
            format!(" primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY}"),
            format!("primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY} "),
            format!("primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY}\t"),
            // A carriage return anywhere but before the line feed.
            format!("primary\r8F1A2B3C4D5E6F70 {PRIMARY_KEY}"),
            format!("primary 8F1A2B3C4D5E6F70 {PRIMARY_KEY}\r\r"),
            format!("primary 8F1A2B3C4D5E6F70 RWQ-{PRIMARY_KEY}"),
            "primary 8F1A2B3C4D5E6F70 RWQ\u{0}x".to_string(),
            "revoked".to_string(),
            "revoked 1111222233334444 1111222233334444".to_string(),
            "revoked 111122223333444".to_string(),
            "revoke 1111222233334444".to_string(),
            " # indented comment".to_string(),
            "\u{feff}# comment after a byte order mark".to_string(),
            "primary\u{a0}8F1A2B3C4D5E6F70 RWQ".to_string(),
        ];
        for bad in &bad_lines {
            let text = format!("# header\n{good}\n\n{bad}\n{good}\n");
            assert_eq!(
                parse_anchors(&text),
                Err(KeyError::BadAnchorsLine { line: 4 }),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_valid_file_builds_trust_anchors() {
        let primary = keypair();
        let backup = keypair();
        let text = format!(
            "# MKLM update trust anchors (design m5b B.2).\n\
             primary {} {}\n\
             backup  {} {}\n\
             revoked 1111222233334444\n\
             revoked 1111222233334444\n",
            id_of(&primary),
            key_text(&primary),
            id_of(&backup),
            key_text(&backup),
        );
        let anchors = TrustAnchors::from_file(&parse_anchors(&text).unwrap()).unwrap();
        assert_eq!(
            anchors.ids(),
            vec![
                (id_of(&primary), KeyRole::Primary),
                (id_of(&backup), KeyRole::Backup)
            ]
        );
        assert_eq!(anchors.role_of(id_of(&backup)), Some(KeyRole::Backup));
        assert_eq!(anchors.role_of(KeyId([9; 8])), None);
        assert_eq!(
            anchors.fingerprint_of(id_of(&primary)),
            Some(public_key_fingerprint(&key_text(&primary)).unwrap())
        );
        assert_eq!(
            anchors.revoked_ids(),
            vec![KeyId::parse("1111222233334444").unwrap()]
        );
        assert!(anchors.revoked_by_build(KeyId::parse("1111222233334444").unwrap()));
        assert!(!anchors.is_dev_key(id_of(&primary)));
        assert_eq!(anchors.check_release_roles(), Ok(()));
        // Debug shows the IDs and roles only.
        let debug = format!("{anchors:?}");
        assert!(debug.contains(&id_of(&primary).to_text()), "{debug}");
        assert!(!debug.contains(&key_text(&primary)), "{debug}");
    }

    #[test]
    fn key_checks_of_the_anchors_file() {
        let a = keypair();
        let b = keypair();
        let line = |role: &str, id: KeyId, key: &str| format!("{role} {id} {key}\n");
        // Only comments: no key.
        assert_eq!(
            TrustAnchors::from_file(&parse_anchors("# nothing\n").unwrap()).map(|_| ()),
            Err(KeyError::NotConfigured)
        );
        assert_eq!(
            TrustAnchors::from_file(&parse_anchors("revoked 1111222233334444\n").unwrap())
                .map(|_| ()),
            Err(KeyError::NotConfigured)
        );
        // The stated ID is not the key's.
        let text =
            line("primary", id_of(&a), &key_text(&a)) + &line("backup", id_of(&a), &key_text(&b));
        assert_eq!(
            TrustAnchors::from_file(&parse_anchors(&text).unwrap()).map(|_| ()),
            Err(KeyError::IdMismatch { index: 1 })
        );
        // The same key twice.
        let text =
            line("primary", id_of(&a), &key_text(&a)) + &line("backup", id_of(&a), &key_text(&a));
        assert_eq!(
            TrustAnchors::from_file(&parse_anchors(&text).unwrap()).map(|_| ()),
            Err(KeyError::DuplicateId)
        );
        // Not a key (the base64 of a signature's first bytes, a truncated key).
        let text = line("primary", id_of(&a), &key_text(&a))
            + &line("backup", id_of(&b), &key_text(&b)[..52]);
        assert_eq!(
            TrustAnchors::from_file(&parse_anchors(&text).unwrap()).map(|_| ()),
            Err(KeyError::BadPublicKey { index: 1 })
        );
        assert_eq!(
            TrustAnchors::from_keys(&[(KeyRole::Primary, "RWQ=")], &[]).map(|_| ()),
            Err(KeyError::BadPublicKey { index: 0 })
        );
        assert_eq!(
            TrustAnchors::from_keys(&[(KeyRole::Primary, &key_text(&a))], &["1"]).map(|_| ()),
            Err(KeyError::BadKeyId {
                text: "1".to_string()
            })
        );
    }

    #[test]
    fn release_roles_are_one_primary_and_one_backup_none_revoked() {
        let [p1, p2, b1] = [keypair(), keypair(), keypair()];
        let anchors = |keys: &[(KeyRole, &minisign::KeyPair)], revoked: &[KeyId]| {
            let texts: Vec<(KeyRole, String)> = keys
                .iter()
                .map(|(role, pair)| (*role, key_text(pair)))
                .collect();
            let keys: Vec<(KeyRole, &str)> = texts
                .iter()
                .map(|(role, text)| (*role, text.as_str()))
                .collect();
            let revoked: Vec<String> = revoked.iter().map(|id| id.to_text()).collect();
            let revoked: Vec<&str> = revoked.iter().map(String::as_str).collect();
            TrustAnchors::from_keys(&keys, &revoked).unwrap()
        };
        use KeyRole::{Backup, Primary};
        assert_eq!(
            anchors(&[(Primary, &p1), (Backup, &b1)], &[]).check_release_roles(),
            Ok(())
        );
        for (keys, revoked) in [
            (vec![(Primary, &p1), (Primary, &p2)], vec![]),
            (vec![(Primary, &p1)], vec![]),
            (vec![(Backup, &b1)], vec![]),
            (vec![(Primary, &p1), (Primary, &p2), (Backup, &b1)], vec![]),
            (vec![(Primary, &p1), (Backup, &b1)], vec![id_of(&p1)]),
            (vec![(Primary, &p1), (Backup, &b1)], vec![id_of(&b1)]),
        ] {
            assert_eq!(
                anchors(&keys, &revoked).check_release_roles(),
                Err(KeyError::Roles),
                "{keys:?} {revoked:?}",
                keys = keys.iter().map(|(r, p)| (r, id_of(p))).collect::<Vec<_>>()
            );
        }
        // A revoked line for a key that is not embedded does not matter.
        assert_eq!(
            anchors(&[(Primary, &p1), (Backup, &b1)], &[id_of(&p2)]).check_release_roles(),
            Ok(())
        );
    }

    #[test]
    fn signature_files_are_read_strictly() {
        let pair = keypair();
        let text = minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(b"data"),
            Some("mklm-latest-json v1 version=0.2.1"),
            None,
        )
        .unwrap()
        .into_string();
        assert_eq!(signature_key_id(&text), Ok(id_of(&pair)));
        let parsed = parse_signature_text(&text).unwrap();
        assert!(parsed.is_prehashed());
        assert_eq!(parsed.trusted_comment, "mklm-latest-json v1 version=0.2.1");
        // CRLF lines and a missing final line break are the same file.
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(parse_signature_text(&crlf), Some(parsed.clone()));
        assert_eq!(
            parse_signature_text(text.strip_suffix('\n').unwrap()),
            Some(parsed.clone())
        );
        let lines: Vec<&str> = text.lines().collect();
        for bad in [
            String::new(),
            format!("{text}\n"),
            format!("{text}extra\n"),
            lines[..3].join("\n"),
            format!(
                "{}\n{}\n{}\n{}\n",
                lines[0],
                lines[1],
                lines[2].replace("trusted comment: ", "trusted comment:"),
                lines[3]
            ),
            format!("{}\n {}\n{}\n{}\n", lines[0], lines[1], lines[2], lines[3]),
            format!("{}\n{}\n{}\n{}=\n", lines[0], lines[1], lines[2], lines[3]),
            format!(
                "{}\n{}\r\r\n{}\n{}\n",
                lines[0], lines[1], lines[2], lines[3]
            ),
            text.replace('\n', "\r"),
        ] {
            assert_eq!(parse_signature_text(&bad), None, "{bad:?}");
            assert_eq!(
                signature_key_id(&bad),
                Err(KeyError::BadSignatureText),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn debug_prints_ids_and_roles_only() {
        let anchors = TrustAnchors {
            keys: vec![AnchorKey {
                id: KeyId::parse("8F1A2B3C4D5E6F70").unwrap(),
                role: Some(KeyRole::Primary),
                fingerprint: KeyFingerprint([7; 32]),
                public_key: minisign_verify::PublicKey::from_base64(PRIMARY_KEY).unwrap(),
            }],
            revoked: vec![KeyId::parse("1111222233334444").unwrap()],
        };
        let text = format!("{anchors:?}");
        assert!(
            text.contains("8F1A2B3C4D5E6F70") && text.contains("Primary"),
            "{text}"
        );
        assert!(text.contains("1111222233334444"), "{text}");
        assert!(
            !text.contains(PRIMARY_KEY) && !text.contains("7, 7"),
            "{text}"
        );
        assert_eq!(
            anchors.ids(),
            vec![(KeyId::parse("8F1A2B3C4D5E6F70").unwrap(), KeyRole::Primary)]
        );
        assert!(anchors.revoked_by_build(KeyId::parse("1111222233334444").unwrap()));
        assert!(!anchors.is_dev_key(KeyId::parse("8F1A2B3C4D5E6F70").unwrap()));
    }

    #[cfg(all(debug_assertions, mklm_update_dev))]
    #[test]
    fn the_development_key_has_no_role() {
        let primary = keypair();
        let dev = keypair();
        let anchors = TrustAnchors::from_keys(&[(KeyRole::Primary, &key_text(&primary))], &[])
            .unwrap()
            .with_dev_key(&key_text(&dev))
            .unwrap();
        assert!(anchors.is_dev_key(id_of(&dev)));
        assert!(!anchors.is_dev_key(id_of(&primary)));
        assert_eq!(anchors.role_of(id_of(&dev)), None);
        assert_eq!(anchors.ids(), vec![(id_of(&primary), KeyRole::Primary)]);
        assert!(anchors.fingerprint_of(id_of(&dev)).is_some());
        let again = anchors.clone().with_dev_key(&key_text(&primary));
        assert_eq!(again.map(|_| ()), Err(KeyError::DuplicateId));
        assert_eq!(
            anchors.with_dev_key("RWQ=").map(|_| ()),
            Err(KeyError::BadPublicKey { index: 2 })
        );
    }
}
