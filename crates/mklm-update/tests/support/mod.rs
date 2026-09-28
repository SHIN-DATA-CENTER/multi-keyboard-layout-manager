//! Shared helpers of the mklm-update integration tests: throwaway minisign keys (generated in the
//! test process, never stored), manifests in the canonical form, and their signatures.

#![allow(dead_code)] // Each test file uses a part.

use std::io::Cursor;

use mklm_update::{
    Arch, KeyId, KeyRole, Manifest, ManifestAsset, Sha256Digest, TRUSTED_COMMENT_PREFIX,
    TrustAnchors, Version, installer_name,
};

/// 2026-10-15 00:00 UTC (design m5b A.2).
pub const T0: u64 = 1_792_022_400;
pub const DAY: u64 = 86_400;

/// A throwaway minisign key pair.
pub struct Key {
    pub pair: minisign::KeyPair,
    pub id: KeyId,
    /// The base64 line of the public key.
    pub public: String,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Key({})", self.id)
    }
}

impl Key {
    pub fn new() -> Key {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().expect("a throwaway key");
        let id = KeyId(pair.pk.keynum().try_into().expect("8 bytes"));
        let public = pair.pk.to_base64();
        Key { pair, id, public }
    }

    /// A prehashed minisign signature file of `data`.
    pub fn sign(&self, data: &[u8], trusted_comment: &str) -> Vec<u8> {
        minisign::sign(
            Some(&self.pair.pk),
            &self.pair.sk,
            Cursor::new(data),
            Some(trusted_comment),
            None,
        )
        .expect("sign")
        .into_string()
        .into_bytes()
    }
}

/// `TrustAnchors::from_keys` over keys and revoked keys.
pub fn anchors(keys: &[(KeyRole, &Key)], revoked: &[&Key]) -> TrustAnchors {
    anchors_with_ids(keys, &revoked.iter().map(|key| key.id).collect::<Vec<_>>())
}

pub fn anchors_with_ids(keys: &[(KeyRole, &Key)], revoked: &[KeyId]) -> TrustAnchors {
    let keys: Vec<(KeyRole, &str)> = keys
        .iter()
        .map(|(role, key)| (*role, key.public.as_str()))
        .collect();
    let revoked: Vec<String> = revoked.iter().map(|id| id.to_text()).collect();
    let revoked: Vec<&str> = revoked.iter().map(String::as_str).collect();
    TrustAnchors::from_keys(&keys, &revoked).expect("valid test anchors")
}

/// The assets of `version`: both architectures, with made-up sizes and digests.
pub fn assets(version: &str) -> Vec<ManifestAsset> {
    let parsed = Version::parse(version).unwrap_or(Version::new(0, 0, 0));
    [(Arch::X64, 6_291_456), (Arch::Arm64, 6_029_312)]
        .into_iter()
        .map(|(arch, size)| {
            let name = installer_name(&parsed, arch);
            ManifestAsset {
                arch,
                sha256: Sha256Digest::of(name.as_bytes()).to_hex(),
                name,
                size,
            }
        })
        .collect()
}

/// A manifest before signing.
#[derive(Debug, Clone)]
pub struct Draft {
    pub manifest: Manifest,
}

impl Draft {
    /// Stable, MKLM, schema 1, expires 180 days after `issued_at`.
    pub fn new(version: &str, issued_at: u64, key_ids: &[&Key]) -> Draft {
        Draft {
            manifest: Manifest {
                schema: 1,
                product: "MKLM".to_string(),
                channel: "stable".to_string(),
                version: version.to_string(),
                issued_at,
                expires: issued_at + 180 * DAY,
                key_ids: key_ids.iter().map(|key| key.id.to_text()).collect(),
                revoked_keys: Vec::new(),
                min_from_version: None,
                assets: assets(version),
            },
        }
    }

    pub fn revoking(mut self, keys: &[KeyId]) -> Draft {
        self.manifest.revoked_keys = keys.iter().map(|id| id.to_text()).collect();
        self
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.manifest.to_canonical_json().into_bytes()
    }

    /// The production trusted comment xtask writes.
    pub fn comment(&self) -> String {
        format!(
            "{TRUSTED_COMMENT_PREFIX} version={} issued_at={}",
            self.manifest.version, self.manifest.issued_at
        )
    }

    pub fn signed_by(&self, key: &Key) -> Signed {
        let manifest = self.bytes();
        let signature = key.sign(&manifest, &self.comment());
        Signed {
            manifest,
            signature,
        }
    }
}

/// A manifest and one signature.
#[derive(Debug, Clone)]
pub struct Signed {
    pub manifest: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Signs raw bytes (malformed manifests that must still verify).
pub fn sign_raw(key: &Key, manifest: &[u8]) -> Signed {
    Signed {
        manifest: manifest.to_vec(),
        signature: key.sign(manifest, TRUSTED_COMMENT_PREFIX),
    }
}
