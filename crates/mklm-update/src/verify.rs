//! `verify_manifest` (design m5b C.3): the same function in the GUI, the CLI, the helper and xtask.
//!
//! WP-0 implements [`SignatureSlot::file_name`]; WP-U the verification.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::keys::{KeyFingerprint, KeyId, KeyRole, TrustAnchors};
use crate::manifest::{Arch, Sha256Digest};
use crate::refusal::UpdateRefusal;
use crate::state::TrustState;
use crate::{ALT_SIGNATURE_NAME, SIGNATURE_NAME, Version};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// GUI / CLI / RecordTrust: an older or equal version is `OfferKind::UpToDate`, not an error.
    Check,
    /// Helper: anything but `OfferKind::Newer` is refused.
    Install,
}

/// Which signature file verified (design m5b A.4, A.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureSlot {
    Main,
    Alt,
}

impl SignatureSlot {
    /// `SIGNATURE_NAME` / `ALT_SIGNATURE_NAME`.
    pub fn file_name(self) -> &'static str {
        match self {
            SignatureSlot::Main => SIGNATURE_NAME,
            SignatureSlot::Alt => ALT_SIGNATURE_NAME,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VerifyInput<'a> {
    pub manifest: &'a [u8],
    /// One signature file (main or alternate).
    pub signature: &'a [u8],
    pub anchors: &'a TrustAnchors,
    pub state: &'a TrustState,
    pub installed: &'a Version,
    pub arch: Arch,
    pub now_unix: u64,
    /// The tag from the fetch's first redirect (design m5b A.6); `None` in the helper.
    pub tag: Option<&'a str>,
    pub purpose: Purpose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAsset {
    pub arch: Arch,
    pub name: String,
    pub size: u64,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferKind {
    Newer,
    UpToDate,
    /// `min_from_version` is newer than the installed version: update by hand.
    ManualRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub version: Version,
    pub issued_at: u64,
    pub expires: u64,
    pub signer: KeyId,
    pub signer_role: KeyRole,
    pub signer_fingerprint: KeyFingerprint,
    /// Signed by the development key (development builds only): records nothing.
    pub signer_is_dev: bool,
    pub key_ids: Vec<KeyId>,
    /// The revocations this build records: embedded primary keys other than the signer, with
    /// their fingerprints (design m5b B.2 rule 2). Unknown IDs, and the embedded backup key in a
    /// primary-signed manifest (ignored, FIX-VERIFICATION-1), are not here.
    pub revoked: BTreeMap<KeyId, KeyFingerprint>,
    pub min_from_version: Option<Version>,
    pub asset: SelectedAsset,
    pub freshness: Freshness,
    pub offer: OfferKind,
}

/// Design m5b C.3, in that order. Never parses the manifest before the signature is verified.
pub fn verify_manifest(input: &VerifyInput<'_>) -> Result<VerifiedManifest, UpdateRefusal> {
    Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
}

/// True for `UnknownKey` and `RevokedKey` only: the caller may then fetch and try the alternate
/// signature (design m5b A.5, C.3).
pub fn tries_alternate(error: &UpdateRefusal) -> bool {
    false // Skeleton (M5b): WP-U
}

/// The helper's handling of `CallerMessage::RecordTrust` (design m5b C.4): verifies with
/// `Purpose::Check` against `machine`, and returns the merged record when it changes anything
/// (`Ok(None)`: verified, nothing new).
pub fn apply_trust_report(
    manifest: &[u8],
    signature: &[u8],
    anchors: &TrustAnchors,
    installed: &Version,
    arch: Arch,
    machine: &TrustState,
    now_unix: u64,
) -> Result<Option<TrustState>, UpdateRefusal> {
    Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_file_names() {
        assert_eq!(SignatureSlot::Main.file_name(), "latest.json.minisig");
        assert_eq!(SignatureSlot::Alt.file_name(), "latest.json.alt.minisig");
        assert_eq!(
            serde_json::to_string(&SignatureSlot::Alt).unwrap(),
            "\"alt\""
        );
        assert_eq!(
            serde_json::from_str::<SignatureSlot>("\"main\"").unwrap(),
            SignatureSlot::Main
        );
    }
}
