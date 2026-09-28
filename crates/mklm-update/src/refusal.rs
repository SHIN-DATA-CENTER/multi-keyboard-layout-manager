//! Every reason an update is not verified, staged or started (design m5b C.3, D.4, D.7, E.6).

use serde::{Deserialize, Serialize};

use crate::manifest::Arch;

/// Why an update was not verified, staged or started. Nothing was changed when one is returned.
/// Serialized inside the pipe's `UpdateMessage::{Refused, TrustNotRecorded}` and the registry's
/// `LastResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "kebab-case")]
pub enum UpdateRefusal {
    // Set-up
    #[error("this build has no update keys")]
    NotConfigured,
    #[error("MKLM does not run from its installation folder")]
    NotInstalledCopy,
    // Sizes
    #[error("the manifest is too large ({len} bytes)")]
    ManifestTooLarge { len: u64 },
    #[error("the signature is too large ({len} bytes)")]
    SignatureTooLarge { len: u64 },
    // Signature
    #[error("the signature file is malformed")]
    SignatureMalformed,
    #[error("the signature's trusted comment is not for an MKLM manifest")]
    WrongTrustedComment,
    #[error("signed with an unknown key {key_id}")]
    UnknownKey { key_id: String },
    #[error("the signature does not verify")]
    BadSignature,
    #[error("signed with the revoked key {key_id}")]
    RevokedKey { key_id: String },
    // Content
    #[error("malformed manifest: {detail}")]
    ManifestMalformed { detail: String },
    #[error("manifest schema {schema} is not supported")]
    UnsupportedSchema { schema: u32 },
    #[error("the manifest is for another product")]
    WrongProduct,
    #[error("the manifest is for another channel")]
    WrongChannel,
    /// Replaces the earlier `KeyIdMismatch` (key_id → key_ids, SECURITY-4).
    #[error("the manifest does not list its signing key {key_id}")]
    SignerNotListed { key_id: String },
    #[error("the manifest may not revoke key {key_id}")]
    IllegalRevocation { key_id: String },
    #[error("{text:?} is not a release version")]
    BadVersion { text: String },
    #[error("the manifest of tag {tag} is for version {version}")]
    TagMismatch { tag: String, version: String },
    #[error("issued_at and expires are inconsistent")]
    BadTimestamps,
    #[error("the manifest was issued at {issued_at}, before {seen}")]
    Rollback { issued_at: u64, seen: u64 },
    #[error("no installer for {arch:?}")]
    NoAssetForArch { arch: Arch },
    #[error("malformed asset: {detail}")]
    AssetMalformed { detail: String },
    // Offer
    #[error("{offered} is not newer than {installed}")]
    NotNewer { offered: String, installed: String },
    #[error("{installed} must be updated by hand (the update requires at least {min_from})")]
    ManualUpdateRequired { min_from: String, installed: String },
    // Helper (staging and run)
    #[error("another MKLM holds the write lock")]
    Busy,
    #[error("another update is in progress")]
    UpdateInProgress,
    #[error("an operation is waiting for the user")]
    OperationOpen { waiting_for_reboot: bool },
    #[error("an interrupted operation needs recovery")]
    RecoveryNeeded,
    #[error("the journal cannot be read")]
    JournalUnreadable,
    #[error("not enough disk space: {needed} bytes needed, {available} available")]
    DiskFull { needed: u64, available: u64 },
    #[error("received {received} of {expected} installer bytes")]
    InstallerSizeMismatch { expected: u64, received: u64 },
    #[error("the installer's SHA-256 does not match the manifest")]
    InstallerHashMismatch,
    #[error("a malformed installer chunk")]
    ChunkMalformed,
    #[error("installer chunk at {found}, expected {expected}")]
    ChunkOutOfOrder { expected: u64, found: u64 },
    #[error("the caller left")]
    CallerLeft,
    #[error("the update runner did not start: {detail}")]
    HandOffFailed { detail: String },
    #[error("storage: {detail}")]
    Storage { detail: String },
    #[error("internal: {detail}")]
    Internal { detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tag and field names as the pipe and `LastResult` carry them (design m5b H.5).
    #[test]
    fn wire_shape() {
        let cases = [
            (
                UpdateRefusal::Rollback {
                    issued_at: 1_792_022_400,
                    seen: 1_792_108_800,
                },
                r#"{"code":"rollback","issued_at":1792022400,"seen":1792108800}"#,
            ),
            (
                UpdateRefusal::OperationOpen {
                    waiting_for_reboot: true,
                },
                r#"{"code":"operation-open","waiting_for_reboot":true}"#,
            ),
            (UpdateRefusal::NotConfigured, r#"{"code":"not-configured"}"#),
            (
                UpdateRefusal::NoAssetForArch { arch: Arch::Arm64 },
                r#"{"code":"no-asset-for-arch","arch":"arm64"}"#,
            ),
            (
                UpdateRefusal::HandOffFailed {
                    detail: "exit code 7".to_string(),
                },
                r#"{"code":"hand-off-failed","detail":"exit code 7"}"#,
            ),
        ];
        for (refusal, json) in cases {
            assert_eq!(serde_json::to_string(&refusal).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<UpdateRefusal>(json).unwrap(),
                refusal
            );
        }
        assert!(serde_json::from_str::<UpdateRefusal>(r#"{"code":"reboot-now"}"#).is_err());
    }
}
