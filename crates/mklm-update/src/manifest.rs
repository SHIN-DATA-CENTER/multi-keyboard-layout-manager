//! `latest.json`, schema 1 (design m5b A.2, A.3), and SHA-256 digests.
//!
//! WP-0 implements [`Arch`]; WP-U the strict parser, the canonical form and the digests.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use serde::{Deserialize, Serialize};

use crate::refusal::UpdateRefusal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X64,   // "x64"
    Arm64, // "arm64"
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
        }
    }

    /// `cfg!(target_arch = "aarch64")` → `Arm64`, else `X64`.
    pub fn of_this_build() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X64
        }
    }
}

/// latest.json, schema 1, as on the wire (design m5b A.2). Checked by `verify_manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub product: String,
    pub channel: String,
    pub version: String,
    pub issued_at: u64,
    pub expires: u64,
    /// 1..=MAX_KEY_IDS distinct key IDs; the main signature's key first (SECURITY-4).
    pub key_ids: Vec<String>,
    pub revoked_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_from_version: Option<String>,
    pub assets: Vec<ManifestAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAsset {
    pub arch: Arch,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    /// Strict: UTF-8 without BOM, no unknown or duplicate fields, nothing after the object.
    /// `ManifestTooLarge` / `ManifestMalformed`.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, UpdateRefusal> {
        Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
    }

    /// The canonical text xtask writes (design m5b A.3).
    pub fn to_canonical_json(&self) -> String {
        String::new() // Skeleton (M5b): WP-U
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest(pub [u8; 32]);

impl Sha256Digest {
    /// Exactly 64 lower-case hex digits.
    pub fn parse_hex(text: &str) -> Option<Sha256Digest> {
        None // Skeleton (M5b): WP-U
    }

    pub fn to_hex(&self) -> String {
        String::new() // Skeleton (M5b): WP-U
    }

    pub fn of(bytes: &[u8]) -> Sha256Digest {
        Sha256Digest([0; 32]) // Skeleton (M5b): WP-U
    }
}

/// Incremental SHA-256 (sha2).
#[derive(Debug, Clone, Default)]
pub struct Sha256Stream {
    hasher: sha2::Sha256,
}

impl Sha256Stream {
    pub fn new() -> Sha256Stream {
        Sha256Stream::default()
    }

    pub fn update(&mut self, bytes: &[u8]) {
        // Skeleton (M5b): WP-U (`sha2::Digest::update`).
    }

    pub fn finish(self) -> Sha256Digest {
        Sha256Digest([0; 32]) // Skeleton (M5b): WP-U
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architectures() {
        assert_eq!(Arch::X64.as_str(), "x64");
        assert_eq!(Arch::Arm64.as_str(), "arm64");
        for arch in [Arch::X64, Arch::Arm64] {
            assert_eq!(
                serde_json::to_string(&arch).unwrap(),
                format!("\"{}\"", arch.as_str())
            );
            assert_eq!(
                serde_json::from_str::<Arch>(&format!("\"{}\"", arch.as_str())).unwrap(),
                arch
            );
        }
        for other in ["\"X64\"", "\"amd64\"", "\"arm\"", "\"aarch64\""] {
            assert!(serde_json::from_str::<Arch>(other).is_err(), "{other}");
        }
        let expected = if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X64
        };
        assert_eq!(Arch::of_this_build(), expected);
    }

    /// The field set of design m5b A.2 (the strict parser itself is WP-U's).
    #[test]
    fn manifest_fields_as_on_the_wire() {
        let json = r#"{
  "schema": 1,
  "product": "MKLM",
  "channel": "stable",
  "version": "0.2.1",
  "issued_at": 1792022400,
  "expires": 1807574400,
  "key_ids": ["8F1A2B3C4D5E6F70"],
  "revoked_keys": [],
  "assets": [
    {
      "arch": "x64",
      "name": "MKLM-Setup-0.2.1-x64.exe",
      "size": 6291456,
      "sha256": "3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b"
    },
    {
      "arch": "arm64",
      "name": "MKLM-Setup-0.2.1-arm64.exe",
      "size": 6029312,
      "sha256": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    }
  ]
}"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.min_from_version, None);
        assert_eq!(manifest.assets[1].arch, Arch::Arm64);
        let back: Manifest =
            serde_json::from_str(&serde_json::to_string(&manifest).unwrap()).unwrap();
        assert_eq!(back, manifest);
        assert!(
            !serde_json::to_string(&manifest)
                .unwrap()
                .contains("min_from_version")
        );
        let unknown = json.replacen("\"schema\": 1,", "\"schema\": 1, \"url\": \"x\",", 1);
        assert!(serde_json::from_str::<Manifest>(&unknown).is_err());
        let unknown_in_asset = json.replacen(
            "\"size\": 6291456,",
            "\"size\": 6291456, \"url\": \"x\",",
            1,
        );
        assert!(serde_json::from_str::<Manifest>(&unknown_in_asset).is_err());
    }
}
