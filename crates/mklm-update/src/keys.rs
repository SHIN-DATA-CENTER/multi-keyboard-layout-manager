//! Key IDs, fingerprints, the trust anchors file and the keys a manifest may be signed with
//! (design m5b B.2, C.2).
//!
//! WP-0 implements the grammar ([`KeyId::parse`] and its text form, [`parse_anchors`]); WP-U the
//! key checks (`TrustAnchors::from_file` and the functions that decode base64).

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::fmt;

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
        String::new() // Skeleton (M5b): WP-U
    }

    pub fn parse_hex(text: &str) -> Option<KeyFingerprint> {
        None // Skeleton (M5b): WP-U
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyRole {
    Primary,
    Backup,
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

/// One key of a [`TrustAnchors`] (Skeleton (M5b): the representation is WP-U's to change).
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
    /// The `revoked` lines of the file.
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
        let _development_key = crate::dev::extra_key(); // Skeleton (M5b): WP-U adds the key.
        Self::release()
    }

    /// A file read from another tag (xtask); the same checks.
    pub fn from_file(file: &AnchorsFile) -> Result<TrustAnchors, KeyError> {
        // Skeleton (M5b): WP-U decodes and checks the keys (design m5b C.2).
        Err(KeyError::NotConfigured)
    }

    /// Tests: the same checks.
    pub fn from_keys(keys: &[(KeyRole, &str)], revoked: &[&str]) -> Result<TrustAnchors, KeyError> {
        Err(KeyError::NotConfigured) // Skeleton (M5b): WP-U
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
        Err(KeyError::NotConfigured) // Skeleton (M5b): WP-U
    }

    fn key(&self, id: KeyId) -> Option<&AnchorKey> {
        self.keys.iter().find(|key| key.id == id)
    }
}

/// The key ID inside a minisign public key (base64 line), strict base64.
pub fn public_key_id(public_key_base64: &str) -> Result<KeyId, KeyError> {
    Err(KeyError::NotConfigured) // Skeleton (M5b): WP-U
}

pub fn public_key_fingerprint(public_key_base64: &str) -> Result<KeyFingerprint, KeyError> {
    Err(KeyError::NotConfigured) // Skeleton (M5b): WP-U
}

/// The key ID inside a minisign signature file (its second line), strict base64.
pub fn signature_key_id(signature_text: &str) -> Result<KeyId, KeyError> {
    Err(KeyError::NotConfigured) // Skeleton (M5b): WP-U
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
    use super::*;

    const PRIMARY_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";

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
    fn the_committed_file_has_no_key_yet() {
        assert_eq!(parse_anchors(ANCHORS_TEXT), Ok(AnchorsFile::default()));
        assert!(ANCHORS_TEXT.starts_with("# MKLM update trust anchors (design m5b B.2)."));
        assert_eq!(ANCHORS_REPO_PATH, "crates/mklm-update/trust/anchors.txt");
        // No key: every build of this tree is `NotConfigured` (design m5b B.2).
        assert_eq!(
            TrustAnchors::release().map(|_| ()),
            Err(KeyError::NotConfigured)
        );
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
}
