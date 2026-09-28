//! The third-party APIs design m5b relies on (B.1, C.2, C.5), as pinned in the workspace:
//! `minisign-verify` 0.3.0, `minisign` 0.10.0 (dev-dependency; throwaway keys generated here and
//! never stored), `sha2` 0.11.0 and `semver` 1.0.28. WP-0 recorded that they resolve, compile and
//! behave as the design assumes; WP-U may fold these checks into its own tests.

use std::cmp::Ordering;
use std::io::Cursor;

use mklm_update::{KeyId, TRUSTED_COMMENT_PREFIX, Version};
use sha2::{Digest, Sha256};

fn keypair() -> minisign::KeyPair {
    minisign::KeyPair::generate_unencrypted_keypair().expect("a throwaway key pair")
}

fn sign(pair: &minisign::KeyPair, data: &[u8], trusted_comment: &str) -> String {
    minisign::sign(
        Some(&pair.pk),
        &pair.sk,
        Cursor::new(data),
        Some(trusted_comment),
        None,
    )
    .expect("sign")
    .into_string()
}

/// `minisign` signs prehashed (`ED`) by default, and `minisign-verify` checks it with
/// `allow_legacy = false`, exposing the trusted comment (design m5b A.4, B.1).
#[test]
fn minisign_signatures_verify_with_minisign_verify() {
    let pair = keypair();
    let manifest = b"{\n  \"schema\": 1\n}\n";
    let comment = format!("{TRUSTED_COMMENT_PREFIX} version=0.2.1 issued_at=1792022400");
    let text = sign(&pair, manifest, &comment);
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(
        text.lines()
            .nth(2)
            .is_some_and(|line| line == format!("trusted comment: {comment}"))
    );

    let public_key = minisign_verify::PublicKey::from_base64(&pair.pk.to_base64()).unwrap();
    let signature = minisign_verify::Signature::decode(&text).unwrap();
    assert_eq!(signature.trusted_comment(), comment);
    public_key.verify(manifest, &signature, false).unwrap();

    // One byte changed: the signature no longer verifies.
    let mut tampered = manifest.to_vec();
    tampered[4] ^= 0x01;
    assert!(matches!(
        public_key.verify(&tampered, &signature, false),
        Err(minisign_verify::Error::InvalidSignature)
    ));
    // Another key: refused by key ID before any signature check.
    let other = minisign_verify::PublicKey::from_base64(&keypair().pk.to_base64()).unwrap();
    assert!(matches!(
        other.verify(manifest, &signature, false),
        Err(minisign_verify::Error::UnexpectedKeyId)
    ));
    // A changed trusted comment breaks the global signature.
    let forged = text.replace(&comment, &format!("{TRUSTED_COMMENT_PREFIX} version=9.9.9"));
    let forged = minisign_verify::Signature::decode(&forged).unwrap();
    assert!(matches!(
        public_key.verify(manifest, &forged, false),
        Err(minisign_verify::Error::InvalidSignature)
    ));
}

/// The key ID text of design m5b B.2 is what minisign prints: the 8 bytes as a little-endian u64
/// in 16 upper-case hex digits (the public key file's untrusted comment carries it).
#[test]
fn key_id_text_is_minisigns() {
    let pair = keypair();
    let keynum: [u8; 8] = pair.pk.keynum().try_into().unwrap();
    let id = KeyId(keynum);
    let public_box = pair.pk.to_box().unwrap().into_string();
    let comment = public_box.lines().next().unwrap();
    assert_eq!(
        comment,
        format!("untrusted comment: minisign public key: {id}")
    );
    assert_eq!(KeyId::parse(&id.to_text()), Ok(id));
    // The base64 line decodes to algorithm "Ed", the key ID, and the Ed25519 key (42 bytes).
    let bytes = pair.pk.to_bytes();
    assert_eq!(bytes.len(), 42);
    assert_eq!(&bytes[..2], b"Ed");
    assert_eq!(&bytes[2..10], &keynum);
}

#[test]
fn sha2_digests() {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert_eq!(
        hex(&Sha256::digest(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let mut stream = Sha256::new();
    stream.update(b"a");
    stream.update(b"bc");
    let digest: [u8; 32] = stream.finalize().into();
    assert_eq!(digest.to_vec(), Sha256::digest(b"abc").to_vec());
}

/// SemVer precedence (design m5b C.5): a development pre-release is older than its release, and
/// build metadata does not change precedence (`Ord` would order it).
#[test]
fn semver_precedence() {
    let parse = |text: &str| Version::parse(text).unwrap();
    assert_eq!(
        parse("0.2.0-dev.1").cmp_precedence(&parse("0.2.0")),
        Ordering::Less
    );
    assert_eq!(
        parse("0.2.1").cmp_precedence(&parse("0.2.0")),
        Ordering::Greater
    );
    assert_eq!(
        parse("0.2.1+x").cmp_precedence(&parse("0.2.1")),
        Ordering::Equal
    );
    assert!(Version::parse("v0.2.1").is_err());
    assert!(Version::parse("0.2").is_err());
}
