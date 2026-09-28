//! `latest.json`, schema 1 (design m5b A.2, A.3), and SHA-256 digests.
//!
//! The parser is strict (UTF-8 without a byte order mark, no unknown or duplicate fields, the
//! types of A.2, nothing but whitespace after the object); the value rules (versions, key IDs,
//! assets, timestamps) are `verify_manifest`'s, after the signature. The canonical text is what
//! xtask writes; clients never require it (they verify the bytes they received).

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::MAX_MANIFEST_LEN;
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
        if bytes.len() > MAX_MANIFEST_LEN {
            return Err(UpdateRefusal::ManifestTooLarge {
                len: bytes.len() as u64,
            });
        }
        let malformed = |detail: &str| UpdateRefusal::ManifestMalformed {
            detail: detail.to_string(),
        };
        if bytes.starts_with(b"\xEF\xBB\xBF") {
            return Err(malformed("a byte order mark"));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| malformed("not UTF-8"))?;
        // serde's derived visitor refuses duplicate fields; serde_json refuses trailing data.
        serde_json::from_str::<Manifest>(text).map_err(|error| malformed(&error.to_string()))
    }

    /// The canonical text xtask writes (design m5b A.3): two-space indentation, the fields in the
    /// order of design m5b A.2 (`min_from_version` only when present), string arrays on one line,
    /// each asset object on its own lines, LF line breaks and one final line break.
    pub fn to_canonical_json(&self) -> String {
        let string = |text: &str| serde_json::to_string(text).unwrap_or_default();
        let strings = |items: &[String]| {
            let items: Vec<String> = items.iter().map(|item| string(item)).collect();
            format!("[{}]", items.join(", "))
        };
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"schema\": {},\n", self.schema));
        out.push_str(&format!("  \"product\": {},\n", string(&self.product)));
        out.push_str(&format!("  \"channel\": {},\n", string(&self.channel)));
        out.push_str(&format!("  \"version\": {},\n", string(&self.version)));
        out.push_str(&format!("  \"issued_at\": {},\n", self.issued_at));
        out.push_str(&format!("  \"expires\": {},\n", self.expires));
        out.push_str(&format!("  \"key_ids\": {},\n", strings(&self.key_ids)));
        out.push_str(&format!(
            "  \"revoked_keys\": {},\n",
            strings(&self.revoked_keys)
        ));
        if let Some(min_from) = &self.min_from_version {
            out.push_str(&format!("  \"min_from_version\": {},\n", string(min_from)));
        }
        if self.assets.is_empty() {
            out.push_str("  \"assets\": []\n");
        } else {
            out.push_str("  \"assets\": [\n");
            for (index, asset) in self.assets.iter().enumerate() {
                out.push_str("    {\n");
                out.push_str(&format!(
                    "      \"arch\": {},\n",
                    string(asset.arch.as_str())
                ));
                out.push_str(&format!("      \"name\": {},\n", string(&asset.name)));
                out.push_str(&format!("      \"size\": {},\n", asset.size));
                out.push_str(&format!("      \"sha256\": {}\n", string(&asset.sha256)));
                out.push_str(if index + 1 == self.assets.len() {
                    "    }\n"
                } else {
                    "    },\n"
                });
            }
            out.push_str("  ]\n");
        }
        out.push_str("}\n");
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest(pub [u8; 32]);

impl Sha256Digest {
    /// Exactly 64 lower-case hex digits.
    pub fn parse_hex(text: &str) -> Option<Sha256Digest> {
        parse_lower_hex_32(text).map(Sha256Digest)
    }

    pub fn to_hex(&self) -> String {
        lower_hex(&self.0)
    }

    pub fn of(bytes: &[u8]) -> Sha256Digest {
        Sha256Digest(sha2::Sha256::digest(bytes).into())
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
        self.hasher.update(bytes);
    }

    pub fn finish(self) -> Sha256Digest {
        Sha256Digest(self.hasher.finalize().into())
    }
}

/// Lower-case hex of any bytes.
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0F)] as char);
    }
    out
}

/// Exactly 64 lower-case hex digits.
pub(crate) fn parse_lower_hex_32(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let value = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    let (pairs, _) = digits.as_chunks::<2>();
    for (byte, [high, low]) in out.iter_mut().zip(pairs) {
        *byte = (value(*high)? << 4) | value(*low)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of design m5b A.2, in the canonical form.
    const EXAMPLE: &str = r#"{
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
}
"#;

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

    #[test]
    fn the_example_parses_and_is_canonical() {
        let manifest = Manifest::parse(EXAMPLE.as_bytes()).unwrap();
        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.version, "0.2.1");
        assert_eq!(manifest.key_ids, ["8F1A2B3C4D5E6F70"]);
        assert_eq!(manifest.min_from_version, None);
        assert_eq!(manifest.assets[1].arch, Arch::Arm64);
        assert_eq!(manifest.to_canonical_json(), EXAMPLE);
        // Compact JSON is accepted as well: the canonical form is not required.
        let compact = serde_json::to_string(&manifest).unwrap();
        assert_eq!(Manifest::parse(compact.as_bytes()).unwrap(), manifest);
        assert!(!compact.contains("min_from_version"));
    }

    #[test]
    fn canonical_form_with_the_optional_fields() {
        let mut manifest = Manifest::parse(EXAMPLE.as_bytes()).unwrap();
        manifest.min_from_version = Some("0.2.0".to_string());
        manifest.key_ids.push("0123456789ABCDEF".to_string());
        manifest.revoked_keys = vec![
            "1111222233334444".to_string(),
            "5555666677778888".to_string(),
        ];
        let text = manifest.to_canonical_json();
        assert!(text.contains(
            "  \"key_ids\": [\"8F1A2B3C4D5E6F70\", \"0123456789ABCDEF\"],\n  \"revoked_keys\": [\"1111222233334444\", \"5555666677778888\"],\n  \"min_from_version\": \"0.2.0\",\n  \"assets\": [\n"
        ), "{text}");
        assert_eq!(Manifest::parse(text.as_bytes()).unwrap(), manifest);
        assert!(text.ends_with("}\n") && !text.ends_with("\n\n") && !text.contains('\r'));
        manifest.assets.clear();
        assert!(
            manifest
                .to_canonical_json()
                .ends_with("  \"assets\": []\n}\n")
        );
    }

    fn refused(text: &str) -> UpdateRefusal {
        Manifest::parse(text.as_bytes()).unwrap_err()
    }

    fn is_malformed(refusal: &UpdateRefusal) -> bool {
        matches!(refusal, UpdateRefusal::ManifestMalformed { .. })
    }

    #[test]
    fn strict_parsing() {
        let cases = [
            // Unknown fields, at the top and in an asset.
            EXAMPLE.replacen("\"schema\": 1,", "\"schema\": 1, \"url\": \"x\",", 1),
            EXAMPLE.replacen(
                "\"size\": 6291456,",
                "\"size\": 6291456, \"url\": \"x\",",
                1,
            ),
            // A duplicate field.
            EXAMPLE.replacen("\"schema\": 1,", "\"schema\": 1, \"schema\": 1,", 1),
            EXAMPLE.replacen(
                "\"size\": 6291456,",
                "\"size\": 6291456, \"size\": 6291456,",
                1,
            ),
            // Trailing data.
            format!("{EXAMPLE}{{}}"),
            format!("{EXAMPLE}x"),
            // Wrong types.
            EXAMPLE.replacen("\"schema\": 1,", "\"schema\": \"1\",", 1),
            EXAMPLE.replacen(
                "\"issued_at\": 1792022400,",
                "\"issued_at\": \"1792022400\",",
                1,
            ),
            EXAMPLE.replacen(
                "\"issued_at\": 1792022400,",
                "\"issued_at\": 1792022400.0,",
                1,
            ),
            EXAMPLE.replacen("\"issued_at\": 1792022400,", "\"issued_at\": -1,", 1),
            EXAMPLE.replacen("\"size\": 6291456,", "\"size\": \"6291456\",", 1),
            EXAMPLE.replacen(
                "\"key_ids\": [\"8F1A2B3C4D5E6F70\"],",
                "\"key_ids\": \"8F1A2B3C4D5E6F70\",",
                1,
            ),
            EXAMPLE.replacen("\"arch\": \"x64\"", "\"arch\": \"X64\"", 1),
            // A missing field.
            EXAMPLE.replacen("  \"channel\": \"stable\",\n", "", 1),
            // Not an object.
            "[]".to_string(),
            String::new(),
        ];
        for text in &cases {
            assert!(is_malformed(&refused(text)), "{text}");
        }
        // A byte order mark, and bytes that are not UTF-8.
        let mut with_bom = b"\xEF\xBB\xBF".to_vec();
        with_bom.extend_from_slice(EXAMPLE.as_bytes());
        assert!(is_malformed(&Manifest::parse(&with_bom).unwrap_err()));
        let mut latin1 = EXAMPLE.as_bytes().to_vec();
        latin1[20] = 0xE9;
        assert!(is_malformed(&Manifest::parse(&latin1).unwrap_err()));
        // Trailing whitespace is not data.
        assert!(Manifest::parse(format!("{EXAMPLE}\n\n ").as_bytes()).is_ok());
    }

    #[test]
    fn size_limit() {
        let padded = |len: usize| {
            let mut text = EXAMPLE.trim_end().to_string();
            while text.len() < len {
                text.push(' ');
            }
            text
        };
        assert!(Manifest::parse(padded(MAX_MANIFEST_LEN).as_bytes()).is_ok());
        assert_eq!(
            Manifest::parse(padded(MAX_MANIFEST_LEN + 1).as_bytes()),
            Err(UpdateRefusal::ManifestTooLarge {
                len: MAX_MANIFEST_LEN as u64 + 1
            })
        );
    }

    #[test]
    fn digests() {
        let abc = Sha256Digest::of(b"abc");
        assert_eq!(
            abc.to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Sha256Digest::parse_hex(&abc.to_hex()), Some(abc));
        let mut stream = Sha256Stream::new();
        stream.update(b"a");
        stream.update(b"");
        stream.update(b"bc");
        assert_eq!(stream.finish(), abc);
        assert_eq!(
            Sha256Stream::new().finish().to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let hex = abc.to_hex();
        for bad in [
            hex[1..].to_string(),
            format!("{hex}0"),
            hex.to_uppercase(),
            format!("{}g", &hex[..63]),
            format!(" {}", &hex[1..]),
            String::new(),
        ] {
            assert_eq!(Sha256Digest::parse_hex(&bad), None, "{bad}");
        }
    }
}
