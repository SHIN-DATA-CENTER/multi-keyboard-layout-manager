//! Revocations and the anti-rollback record (design m5b B.2, C.4): the machine record (HKLM
//! `Update\Trust`, written by the helper only) and the user record (`state.json`).
//!
//! WP-0 writes the types; WP-U the rules.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::keys::{KeyFingerprint, KeyId, TrustAnchors};
use crate::verify::VerifiedManifest;

/// One recorded revocation, scoped to the key it was aimed at (SECURITY-3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RevokedKey {
    pub key_id: String,
    /// `KeyFingerprint::to_hex`.
    pub fingerprint: String,
}

/// Revocations and the highest recorded `issued_at` per signing key (design m5b B.2, C.4). JSON;
/// unknown fields are ignored. `Default` has `schema = 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    pub schema: u32,
    /// Key ID text → highest `min(issued_at, time of recording)` seen from that key.
    #[serde(default)]
    pub max_issued_at: BTreeMap<String, u64>,
    #[serde(default)]
    pub revoked: BTreeSet<RevokedKey>,
}

impl Default for TrustState {
    fn default() -> TrustState {
        TrustState {
            schema: 1,
            max_issued_at: BTreeMap::new(),
            revoked: BTreeSet::new(),
        }
    }
}

impl TrustState {
    pub fn parse(json: &str) -> Result<TrustState, StateError> {
        Err(skeleton()) // Skeleton (M5b): WP-U
    }

    pub fn to_json(&self) -> String {
        String::new() // Skeleton (M5b): WP-U
    }

    /// Per key the higher value; the union of the revocations.
    pub fn merged(&self, other: &TrustState) -> TrustState {
        self.clone() // Skeleton (M5b): WP-U
    }

    /// After a successful verification: the signer's maximum becomes
    /// `max(old, min(verified.issued_at, now_unix))`, and `verified.revoked` is added. A
    /// development-key manifest changes nothing (SECURITY-12, RELIABILITY-6).
    pub fn recorded(&self, verified: &VerifiedManifest, now_unix: u64) -> TrustState {
        self.clone() // Skeleton (M5b): WP-U
    }

    /// A recorded revocation with this ID and fingerprint.
    pub fn is_revoked(&self, id: KeyId, fingerprint: KeyFingerprint) -> bool {
        false // Skeleton (M5b): WP-U
    }

    pub fn max_issued_at(&self, id: KeyId) -> Option<u64> {
        None // Skeleton (M5b): WP-U
    }

    /// The anti-rollback threshold (design m5b B.2): the highest recorded value over all keys
    /// except those revoked by the build, by this record (ID and, for keys `anchors` knows, the
    /// fingerprint; unknown keys by ID), or in `revoking` (the manifest being verified).
    pub fn rollback_threshold(
        &self,
        anchors: &TrustAnchors,
        revoking: &BTreeSet<KeyId>,
    ) -> Option<u64> {
        None // Skeleton (M5b): WP-U
    }

    /// A higher maximum for some key, or a revocation `other` lacks: the client then sends
    /// `RecordTrust` (design m5b C.4).
    pub fn is_ahead_of(&self, other: &TrustState) -> bool {
        false // Skeleton (M5b): WP-U
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("malformed update record: {0}")]
    Malformed(String),
    #[error("update record schema {0} is not supported")]
    Schema(u32),
}

/// What the WP-0 skeleton's unimplemented functions return (design m5b G.2).
pub(crate) fn skeleton() -> StateError {
    StateError::Malformed("not implemented (m5b skeleton)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `Trust` value of design m5b H.5, through serde (the parser with its schema check is
    /// WP-U's `TrustState::parse`).
    #[test]
    fn machine_record_shape() {
        let json = r#"{"schema":1,"max_issued_at":{"8F1A2B3C4D5E6F70":1792022400},"revoked":[{"key_id":"1111222233334444","fingerprint":"5d41402abc4b2a76b9719d911017c5925d41402abc4b2a76b9719d911017c592"}]}"#;
        let state: TrustState = serde_json::from_str(json).unwrap();
        assert_eq!(
            state.max_issued_at.get("8F1A2B3C4D5E6F70"),
            Some(&1_792_022_400)
        );
        assert_eq!(state.revoked.len(), 1);
        assert_eq!(serde_json::to_string(&state).unwrap(), json);
        // Unknown fields are ignored; the maps default to empty.
        let lenient: TrustState = serde_json::from_str(r#"{"schema":1,"future":true}"#).unwrap();
        assert_eq!(lenient, TrustState::default());
        assert_eq!(TrustState::default().schema, 1);
    }
}
