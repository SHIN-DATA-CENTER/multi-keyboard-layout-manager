//! `verify_manifest` (design m5b C.3): the same function in the GUI, the CLI, the helper and xtask.
//!
//! The steps run in the order of C.3 and stop at the first failure. Up to step 6 (the signature)
//! only the signature file is read; the manifest bytes are first parsed in step 7, after they
//! verified. The key rules (steps 3, 4 and 10) are also offered on key IDs alone
//! ([`check_key_rules`]) for xtask's strand check (design m5b B.3 step 10).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::keys::{KeyFingerprint, KeyId, KeyRole, TrustAnchors, parse_signature_text};
use crate::manifest::{Arch, Manifest, Sha256Digest};
use crate::refusal::UpdateRefusal;
use crate::state::TrustState;
use crate::version::{is_newer, parse_release_version};
use crate::{
    ALT_SIGNATURE_NAME, CHANNEL, DEV_TRUSTED_COMMENT_PREFIX, MANIFEST_SCHEMA, MAX_INSTALLER_LEN,
    MAX_KEY_IDS, MAX_MANIFEST_LEN, MAX_SIGNATURE_LEN, MAX_VALIDITY_SECS, PRODUCT, SIGNATURE_NAME,
    TRUSTED_COMMENT_PREFIX, Version, installer_name,
};

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
    /// The development key (which has no role) reports `Primary`; see `signer_is_dev`.
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

/// The key a signature names, once it passed steps 3 and 4.
struct Signer {
    id: KeyId,
    role: KeyRole,
    fingerprint: KeyFingerprint,
    is_dev: bool,
}

/// Steps 3 and 4: a key of `anchors`, revoked neither by the build (ID) nor by the record (ID and
/// fingerprint).
fn select_signer(
    anchors: &TrustAnchors,
    state: &TrustState,
    id: KeyId,
) -> Result<Signer, UpdateRefusal> {
    let Some(fingerprint) = anchors.fingerprint_of(id) else {
        return Err(UpdateRefusal::UnknownKey {
            key_id: id.to_text(),
        });
    };
    if anchors.revoked_by_build(id) || state.is_revoked(id, fingerprint) {
        return Err(UpdateRefusal::RevokedKey {
            key_id: id.to_text(),
        });
    }
    let is_dev = anchors.is_dev_key(id);
    Ok(Signer {
        id,
        role: anchors.role_of(id).unwrap_or(KeyRole::Primary),
        fingerprint,
        is_dev,
    })
}

/// Step 10, the table of design m5b B.2 rule 2: which of `revoked_keys` this build records.
///
/// - the signer itself: `IllegalRevocation` (the whole manifest is refused);
/// - an ID the build already revokes: nothing;
/// - another embedded primary key: recorded, with its fingerprint;
/// - an embedded backup key: ignored (a backup key is revoked only by a later build's anchors
///   file; FIX-VERIFICATION-1);
/// - an ID the build does not embed: ignored (SECURITY-3);
/// - everything in a manifest signed by the development key: ignored.
fn revocations_to_record(
    anchors: &TrustAnchors,
    signer: &Signer,
    revoked_keys: &[KeyId],
) -> Result<BTreeMap<KeyId, KeyFingerprint>, UpdateRefusal> {
    let mut recorded = BTreeMap::new();
    if signer.is_dev {
        return Ok(recorded);
    }
    for &id in revoked_keys {
        if id == signer.id {
            return Err(UpdateRefusal::IllegalRevocation {
                key_id: id.to_text(),
            });
        }
        if anchors.revoked_by_build(id) {
            continue;
        }
        if anchors.role_of(id) == Some(KeyRole::Primary)
            && let Some(fingerprint) = anchors.fingerprint_of(id)
        {
            recorded.insert(id, fingerprint);
        }
    }
    Ok(recorded)
}

/// Design m5b C.3 steps 3, 4 and 10 on key IDs alone, with an empty record: whether a build whose
/// trust anchors are `anchors` would accept a manifest signed by `signer` that revokes
/// `revoked_keys` (as far as the keys are concerned). xtask's strand check (design m5b B.3
/// step 10) runs it for every release in the transition window.
pub fn check_key_rules(
    anchors: &TrustAnchors,
    signer: KeyId,
    revoked_keys: &[KeyId],
) -> Result<(), UpdateRefusal> {
    let signer = select_signer(anchors, &TrustState::default(), signer)?;
    revocations_to_record(anchors, &signer, revoked_keys).map(|_| ())
}

/// The trusted comment starts with `prefix`, followed by the end of the line or white space
/// (design m5b A.4).
fn has_prefix(comment: &str, prefix: &str) -> bool {
    comment
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

fn malformed(detail: impl Into<String>) -> UpdateRefusal {
    UpdateRefusal::ManifestMalformed {
        detail: detail.into(),
    }
}

fn asset_malformed(detail: impl Into<String>) -> UpdateRefusal {
    UpdateRefusal::AssetMalformed {
        detail: detail.into(),
    }
}

/// Design m5b C.3, in that order. Never parses the manifest before the signature is verified.
pub fn verify_manifest(input: &VerifyInput<'_>) -> Result<VerifiedManifest, UpdateRefusal> {
    // 1. Sizes.
    if input.manifest.len() > MAX_MANIFEST_LEN {
        return Err(UpdateRefusal::ManifestTooLarge {
            len: input.manifest.len() as u64,
        });
    }
    if input.signature.len() > MAX_SIGNATURE_LEN {
        return Err(UpdateRefusal::SignatureTooLarge {
            len: input.signature.len() as u64,
        });
    }
    // 2. The signature file: UTF-8, the strict four-line form, prehashed only (A.4).
    let signature_text =
        std::str::from_utf8(input.signature).map_err(|_| UpdateRefusal::SignatureMalformed)?;
    let parsed = parse_signature_text(signature_text)
        .filter(|parsed| parsed.is_prehashed())
        .ok_or(UpdateRefusal::SignatureMalformed)?;
    let signature = minisign_verify::Signature::decode(signature_text)
        .map_err(|_| UpdateRefusal::SignatureMalformed)?;
    if signature.trusted_comment() != parsed.trusted_comment {
        return Err(UpdateRefusal::SignatureMalformed);
    }
    // 3. and 4. A known key, not revoked.
    let signer = select_signer(input.anchors, input.state, parsed.key_id)?;
    // 5. The trusted comment of the key's kind (production or development; never the drill's).
    let prefix = if signer.is_dev {
        DEV_TRUSTED_COMMENT_PREFIX
    } else {
        TRUSTED_COMMENT_PREFIX
    };
    if !has_prefix(signature.trusted_comment(), prefix) {
        return Err(UpdateRefusal::WrongTrustedComment);
    }
    // 6. The signature; the manifest is still unread.
    let verifier = input
        .anchors
        .verifier(signer.id)
        .ok_or(UpdateRefusal::BadSignature)?;
    verifier
        .verify(input.manifest, &signature, false)
        .map_err(|_| UpdateRefusal::BadSignature)?;
    // 7. Strict parse.
    let manifest = Manifest::parse(input.manifest)?;
    // 8. Schema, product, channel.
    if manifest.schema != MANIFEST_SCHEMA {
        return Err(UpdateRefusal::UnsupportedSchema {
            schema: manifest.schema,
        });
    }
    if manifest.product != PRODUCT {
        return Err(UpdateRefusal::WrongProduct);
    }
    if manifest.channel != CHANNEL {
        return Err(UpdateRefusal::WrongChannel);
    }
    // 9. key_ids: 1 or 2 distinct well-formed IDs, the signer among them.
    if manifest.key_ids.is_empty() || manifest.key_ids.len() > MAX_KEY_IDS {
        return Err(malformed(format!(
            "key_ids holds {} IDs (1 to {MAX_KEY_IDS})",
            manifest.key_ids.len()
        )));
    }
    let key_ids = manifest
        .key_ids
        .iter()
        .map(|text| KeyId::parse(text).map_err(|error| malformed(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    if key_ids.iter().collect::<BTreeSet<_>>().len() != key_ids.len() {
        return Err(malformed("key_ids repeats an ID"));
    }
    if !key_ids.contains(&signer.id) {
        return Err(UpdateRefusal::SignerNotListed {
            key_id: signer.id.to_text(),
        });
    }
    // 10. revoked_keys: well-formed, and the rules of B.2.
    let revoked_keys = manifest
        .revoked_keys
        .iter()
        .map(|text| KeyId::parse(text).map_err(|error| malformed(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let revoked = revocations_to_record(input.anchors, &signer, &revoked_keys)?;
    // 11. Versions, and the tag of the fetch.
    let version = parse_release_version(&manifest.version)?;
    let min_from_version = manifest
        .min_from_version
        .as_deref()
        .map(parse_release_version)
        .transpose()?;
    if let Some(tag) = input.tag
        && tag != format!("v{version}")
    {
        return Err(UpdateRefusal::TagMismatch {
            tag: tag.to_string(),
            version: manifest.version.clone(),
        });
    }
    // 12. Timestamps.
    if manifest.issued_at >= manifest.expires
        || manifest.expires - manifest.issued_at > MAX_VALIDITY_SECS
    {
        return Err(UpdateRefusal::BadTimestamps);
    }
    // 13. Anti-rollback over all keys, except the keys this manifest rightly revokes.
    let revoking: BTreeSet<KeyId> = revoked.keys().copied().collect();
    if let Some(seen) = input.state.rollback_threshold(input.anchors, &revoking)
        && manifest.issued_at < seen
    {
        return Err(UpdateRefusal::Rollback {
            issued_at: manifest.issued_at,
            seen,
        });
    }
    // 14. Assets: exactly one x64 and one arm64, the names of this version, sizes and digests.
    let asset = select_asset(&manifest, &version, input.arch)?;
    // 15. Freshness: advisory only (C.6).
    let freshness = if input.now_unix > manifest.expires {
        Freshness::Expired
    } else {
        Freshness::Fresh
    };
    // 16. The offer.
    let offer = match &min_from_version {
        Some(min_from) if input.installed.cmp_precedence(min_from).is_lt() => {
            OfferKind::ManualRequired
        }
        _ if is_newer(&version, input.installed) => OfferKind::Newer,
        _ => OfferKind::UpToDate,
    };
    // 17. The helper installs newer versions only.
    if input.purpose == Purpose::Install {
        match offer {
            OfferKind::Newer => {}
            OfferKind::UpToDate => {
                return Err(UpdateRefusal::NotNewer {
                    offered: version.to_string(),
                    installed: input.installed.to_string(),
                });
            }
            OfferKind::ManualRequired => {
                return Err(UpdateRefusal::ManualUpdateRequired {
                    min_from: min_from_version
                        .as_ref()
                        .map(Version::to_string)
                        .unwrap_or_default(),
                    installed: input.installed.to_string(),
                });
            }
        }
    }
    Ok(VerifiedManifest {
        version,
        issued_at: manifest.issued_at,
        expires: manifest.expires,
        signer: signer.id,
        signer_role: signer.role,
        signer_fingerprint: signer.fingerprint,
        signer_is_dev: signer.is_dev,
        key_ids,
        revoked,
        min_from_version,
        asset,
        freshness,
        offer,
    })
}

/// Step 14.
fn select_asset(
    manifest: &Manifest,
    version: &Version,
    arch: Arch,
) -> Result<SelectedAsset, UpdateRefusal> {
    let archs: BTreeSet<Arch> = manifest.assets.iter().map(|asset| asset.arch).collect();
    if manifest.assets.len() != 2 || archs != BTreeSet::from([Arch::X64, Arch::Arm64]) {
        return Err(asset_malformed(
            "expected exactly one x64 and one arm64 asset",
        ));
    }
    let mut selected = None;
    for asset in &manifest.assets {
        let expected = installer_name(version, asset.arch);
        if asset.name != expected {
            return Err(asset_malformed(format!(
                "{:?} is not {expected:?}",
                asset.name
            )));
        }
        if asset.size == 0 || asset.size > MAX_INSTALLER_LEN {
            return Err(asset_malformed(format!(
                "{} is {} bytes (1 to {MAX_INSTALLER_LEN})",
                asset.name, asset.size
            )));
        }
        let Some(sha256) = Sha256Digest::parse_hex(&asset.sha256) else {
            return Err(asset_malformed(format!(
                "the sha256 of {} is not 64 lower-case hex digits",
                asset.name
            )));
        };
        if asset.arch == arch {
            selected = Some(SelectedAsset {
                arch,
                name: asset.name.clone(),
                size: asset.size,
                sha256,
            });
        }
    }
    selected.ok_or(UpdateRefusal::NoAssetForArch { arch })
}

/// True for `UnknownKey` and `RevokedKey` only: the caller may then fetch and try the alternate
/// signature (design m5b A.5, C.3).
pub fn tries_alternate(error: &UpdateRefusal) -> bool {
    matches!(
        error,
        UpdateRefusal::UnknownKey { .. } | UpdateRefusal::RevokedKey { .. }
    )
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
    let verified = verify_manifest(&VerifyInput {
        manifest,
        signature,
        anchors,
        state: machine,
        installed,
        arch,
        now_unix,
        tag: None,
        purpose: Purpose::Check,
    })?;
    let recorded = machine.recorded(&verified, now_unix);
    Ok((recorded != *machine).then_some(recorded))
}

/// A minisign signature of any file with one given public key (xtask only: `verify-signer` checks
/// the official minisign release with its author's key, legacy signatures allowed; `key-drill
/// check` checks the drill nonce, prehashed only). The strict signature form of [`verify_manifest`];
/// returns the trusted comment, which the caller checks.
pub fn verify_file_signature(
    public_key_base64: &str,
    data: &[u8],
    signature: &[u8],
    allow_legacy: bool,
) -> Result<String, UpdateRefusal> {
    if signature.len() > MAX_SIGNATURE_LEN {
        return Err(UpdateRefusal::SignatureTooLarge {
            len: signature.len() as u64,
        });
    }
    let text = std::str::from_utf8(signature).map_err(|_| UpdateRefusal::SignatureMalformed)?;
    let parsed = parse_signature_text(text)
        .filter(|parsed| allow_legacy || parsed.is_prehashed())
        .ok_or(UpdateRefusal::SignatureMalformed)?;
    let not_a_key = |_| UpdateRefusal::Internal {
        detail: "not a minisign public key".to_string(),
    };
    let key_id = crate::keys::public_key_id(public_key_base64).map_err(not_a_key)?;
    if parsed.key_id != key_id {
        return Err(UpdateRefusal::UnknownKey {
            key_id: parsed.key_id.to_text(),
        });
    }
    let public_key = minisign_verify::PublicKey::from_base64(public_key_base64).map_err(|_| {
        UpdateRefusal::Internal {
            detail: "not a minisign public key".to_string(),
        }
    })?;
    let decoded =
        minisign_verify::Signature::decode(text).map_err(|_| UpdateRefusal::SignatureMalformed)?;
    public_key
        .verify(data, &decoded, allow_legacy)
        .map_err(|_| UpdateRefusal::BadSignature)?;
    Ok(decoded.trusted_comment().to_string())
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

    #[test]
    fn trusted_comment_prefixes() {
        let p = TRUSTED_COMMENT_PREFIX;
        assert!(has_prefix(p, p));
        assert!(has_prefix(&format!("{p} version=0.2.1 issued_at=1"), p));
        assert!(has_prefix(&format!("{p}\tx"), p));
        assert!(!has_prefix(&format!("{p}x"), p));
        assert!(!has_prefix(&format!("{p}0 version=0.2.1"), p));
        assert!(!has_prefix(&format!(" {p}"), p));
        assert!(!has_prefix("mklm-latest-json", p));
        assert!(!has_prefix(DEV_TRUSTED_COMMENT_PREFIX, p));
        assert!(!has_prefix(crate::KEY_DRILL_COMMENT_PREFIX, p));
        assert!(!has_prefix("", p));
    }

    #[test]
    fn only_key_selection_failures_try_the_alternate() {
        assert!(tries_alternate(&UpdateRefusal::UnknownKey {
            key_id: "1".into()
        }));
        assert!(tries_alternate(&UpdateRefusal::RevokedKey {
            key_id: "1".into()
        }));
        for other in [
            UpdateRefusal::BadSignature,
            UpdateRefusal::SignatureMalformed,
            UpdateRefusal::WrongTrustedComment,
            UpdateRefusal::IllegalRevocation { key_id: "1".into() },
            UpdateRefusal::Rollback {
                issued_at: 1,
                seen: 2,
            },
            UpdateRefusal::ManifestMalformed { detail: "x".into() },
            UpdateRefusal::SignerNotListed { key_id: "1".into() },
        ] {
            assert!(!tries_alternate(&other), "{other:?}");
        }
    }
}
