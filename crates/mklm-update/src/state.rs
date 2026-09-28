//! Revocations and the anti-rollback record (design m5b B.2, C.4): the machine record (HKLM
//! `Update\Trust`, written by the helper only) and the user record (`state.json`).
//!
//! - A recorded revocation is the pair (key ID, fingerprint): it hits a signing key only when both
//!   match, so a later build that trusts another key under a reused ID is not affected
//!   (SECURITY-3).
//! - Per signing key, the record keeps the highest `min(issued_at, time of recording)`: a
//!   manifest dated in the future (a wrong clock, a leaked key) never lifts the record above the
//!   time it was received, so later correct manifests still pass (SECURITY-12, RELIABILITY-6).
//! - The rollback threshold is the highest value over all keys, except the keys revoked by the
//!   build, by the record, or by the manifest being verified (SECURITY-5).

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

impl RevokedKey {
    fn of(id: KeyId, fingerprint: KeyFingerprint) -> RevokedKey {
        RevokedKey {
            key_id: id.to_text(),
            fingerprint: fingerprint.to_hex(),
        }
    }
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
    /// The record's JSON (design m5b H.5). Unknown fields are ignored; `schema` must be 1; every
    /// key ID must be `KeyId::parse`'s form and every fingerprint 64 lower-case hex digits
    /// (`Malformed` otherwise: the record is ours, anything else is damage).
    pub fn parse(json: &str) -> Result<TrustState, StateError> {
        let state: TrustState =
            serde_json::from_str(json).map_err(|error| StateError::Malformed(error.to_string()))?;
        if state.schema != 1 {
            return Err(StateError::Schema(state.schema));
        }
        for id in state.max_issued_at.keys() {
            KeyId::parse(id).map_err(|error| StateError::Malformed(error.to_string()))?;
        }
        for revoked in &state.revoked {
            KeyId::parse(&revoked.key_id)
                .map_err(|error| StateError::Malformed(error.to_string()))?;
            if KeyFingerprint::parse_hex(&revoked.fingerprint).is_none() {
                return Err(StateError::Malformed(format!(
                    "{:?} is not a key fingerprint",
                    revoked.fingerprint
                )));
            }
        }
        Ok(state)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Per key the higher value; the union of the revocations.
    pub fn merged(&self, other: &TrustState) -> TrustState {
        let mut merged = self.clone();
        merged.schema = 1;
        for (id, &value) in &other.max_issued_at {
            let entry = merged.max_issued_at.entry(id.clone()).or_insert(value);
            *entry = (*entry).max(value);
        }
        merged.revoked.extend(other.revoked.iter().cloned());
        merged
    }

    /// After a successful verification: the signer's maximum becomes
    /// `max(old, min(verified.issued_at, now_unix))`, and `verified.revoked` is added. A
    /// development-key manifest changes nothing (SECURITY-12, RELIABILITY-6).
    pub fn recorded(&self, verified: &VerifiedManifest, now_unix: u64) -> TrustState {
        let mut state = self.clone();
        if verified.signer_is_dev {
            return state;
        }
        let value = verified.issued_at.min(now_unix);
        let entry = state
            .max_issued_at
            .entry(verified.signer.to_text())
            .or_insert(value);
        *entry = (*entry).max(value);
        state.revoked.extend(
            verified
                .revoked
                .iter()
                .map(|(&id, &fingerprint)| RevokedKey::of(id, fingerprint)),
        );
        state
    }

    /// A recorded revocation with this ID and fingerprint.
    pub fn is_revoked(&self, id: KeyId, fingerprint: KeyFingerprint) -> bool {
        self.revoked.contains(&RevokedKey::of(id, fingerprint))
    }

    pub fn max_issued_at(&self, id: KeyId) -> Option<u64> {
        self.max_issued_at.get(&id.to_text()).copied()
    }

    /// The anti-rollback threshold (design m5b B.2): the highest recorded value over all keys
    /// except those revoked by the build, by this record (ID and, for keys `anchors` knows, the
    /// fingerprint; unknown keys by ID), or in `revoking` (the manifest being verified).
    ///
    /// An entry whose key ID text does not parse (never written by this crate) is counted: when
    /// in doubt, the threshold stays high (fail closed).
    pub fn rollback_threshold(
        &self,
        anchors: &TrustAnchors,
        revoking: &BTreeSet<KeyId>,
    ) -> Option<u64> {
        self.max_issued_at
            .iter()
            .filter(|(text, _)| match KeyId::parse(text) {
                Ok(id) => !self.excluded(id, anchors, revoking),
                Err(_) => true,
            })
            .map(|(_, &value)| value)
            .max()
    }

    fn excluded(&self, id: KeyId, anchors: &TrustAnchors, revoking: &BTreeSet<KeyId>) -> bool {
        if anchors.revoked_by_build(id) || revoking.contains(&id) {
            return true;
        }
        match anchors.fingerprint_of(id) {
            Some(fingerprint) => self.is_revoked(id, fingerprint),
            None => {
                let text = id.to_text();
                self.revoked.iter().any(|revoked| revoked.key_id == text)
            }
        }
    }

    /// A higher maximum for some key, or a revocation `other` lacks: the client then sends
    /// `RecordTrust` (design m5b C.4).
    pub fn is_ahead_of(&self, other: &TrustState) -> bool {
        self.max_issued_at.iter().any(|(id, &value)| {
            other
                .max_issued_at
                .get(id)
                .is_none_or(|&theirs| value > theirs)
        }) || !self.revoked.is_subset(&other.revoked)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("malformed update record: {0}")]
    Malformed(String),
    #[error("update record schema {0} is not supported")]
    Schema(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRUST_JSON: &str = r#"{"schema":1,"max_issued_at":{"8F1A2B3C4D5E6F70":1792022400},"revoked":[{"key_id":"1111222233334444","fingerprint":"5d41402abc4b2a76b9719d911017c5925d41402abc4b2a76b9719d911017c592"}]}"#;

    fn id(text: &str) -> KeyId {
        KeyId::parse(text).unwrap()
    }

    /// The `Trust` value of design m5b H.5.
    #[test]
    fn machine_record_shape() {
        let state = TrustState::parse(TRUST_JSON).unwrap();
        assert_eq!(
            state.max_issued_at.get("8F1A2B3C4D5E6F70"),
            Some(&1_792_022_400)
        );
        assert_eq!(
            state.max_issued_at(id("8F1A2B3C4D5E6F70")),
            Some(1_792_022_400)
        );
        assert_eq!(state.max_issued_at(id("1111222233334444")), None);
        assert_eq!(state.revoked.len(), 1);
        assert_eq!(state.to_json(), TRUST_JSON);
        assert_eq!(TrustState::parse(&state.to_json()), Ok(state.clone()));
        let fingerprint = KeyFingerprint::parse_hex(
            "5d41402abc4b2a76b9719d911017c5925d41402abc4b2a76b9719d911017c592",
        )
        .unwrap();
        assert!(state.is_revoked(id("1111222233334444"), fingerprint));
        assert!(!state.is_revoked(id("1111222233334444"), KeyFingerprint([0; 32])));
        assert!(!state.is_revoked(id("8F1A2B3C4D5E6F70"), fingerprint));
        // Unknown fields are ignored; the maps default to empty.
        let lenient = TrustState::parse(r#"{"schema":1,"future":true}"#).unwrap();
        assert_eq!(lenient, TrustState::default());
        assert_eq!(TrustState::default().schema, 1);
        assert_eq!(
            TrustState::default().to_json(),
            r#"{"schema":1,"max_issued_at":{},"revoked":[]}"#
        );
    }

    #[test]
    fn damaged_records() {
        assert_eq!(
            TrustState::parse(r#"{"schema":2}"#),
            Err(StateError::Schema(2))
        );
        for bad in [
            "",
            "{",
            "null",
            "[]",
            r#"{"max_issued_at":{}}"#,
            r#"{"schema":"1"}"#,
            r#"{"schema":1,"max_issued_at":{"8f1a2b3c4d5e6f70":1}}"#,
            r#"{"schema":1,"max_issued_at":{"8F1A2B3C4D5E6F70":-1}}"#,
            r#"{"schema":1,"revoked":[{"key_id":"1111222233334444"}]}"#,
            r#"{"schema":1,"revoked":[{"key_id":"1111222233334444","fingerprint":"00"}]}"#,
            r#"{"schema":1,"revoked":[{"key_id":"x","fingerprint":"5d41402abc4b2a76b9719d911017c5925d41402abc4b2a76b9719d911017c592"}]}"#,
        ] {
            assert!(
                matches!(TrustState::parse(bad), Err(StateError::Malformed(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn merging_takes_maxima_and_the_union() {
        let revoked = |key: &str| RevokedKey {
            key_id: key.to_string(),
            fingerprint: "ab".repeat(32),
        };
        let a = TrustState {
            schema: 1,
            max_issued_at: [
                ("AAAAAAAAAAAAAAAA".to_string(), 10),
                ("BBBBBBBBBBBBBBBB".to_string(), 5),
            ]
            .into(),
            revoked: [revoked("1111111111111111")].into(),
        };
        let b = TrustState {
            schema: 1,
            max_issued_at: [
                ("BBBBBBBBBBBBBBBB".to_string(), 7),
                ("CCCCCCCCCCCCCCCC".to_string(), 1),
            ]
            .into(),
            revoked: [revoked("2222222222222222")].into(),
        };
        let merged = a.merged(&b);
        assert_eq!(merged, b.merged(&a));
        assert_eq!(merged.max_issued_at["AAAAAAAAAAAAAAAA"], 10);
        assert_eq!(merged.max_issued_at["BBBBBBBBBBBBBBBB"], 7);
        assert_eq!(merged.max_issued_at["CCCCCCCCCCCCCCCC"], 1);
        assert_eq!(merged.revoked.len(), 2);
        assert!(merged.is_ahead_of(&a) && merged.is_ahead_of(&b));
        assert!(!a.is_ahead_of(&merged) && !b.is_ahead_of(&merged));
        assert!(!merged.is_ahead_of(&merged));
        assert!(a.is_ahead_of(&b) && b.is_ahead_of(&a));
        assert!(!TrustState::default().is_ahead_of(&a));
        // A revocation alone is ahead.
        let mut only_revoked = b.clone();
        only_revoked.revoked.insert(revoked("3333333333333333"));
        assert!(only_revoked.is_ahead_of(&b));
        // An equal or lower maximum is not.
        let mut lower = b.clone();
        lower
            .max_issued_at
            .insert("BBBBBBBBBBBBBBBB".to_string(), 6);
        assert!(!lower.is_ahead_of(&b));
    }
}
