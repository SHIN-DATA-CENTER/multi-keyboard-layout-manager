//! `verify_manifest`, the records and `apply_trust_report` (design m5b F.1), with throwaway keys
//! (`TrustAnchors::from_keys`; never `for_this_build`, so the developer's environment cannot
//! change the results).

mod support;

use std::collections::BTreeSet;

use mklm_update::keys::public_key_fingerprint;
use mklm_update::{
    Arch, DEV_TRUSTED_COMMENT_PREFIX, Freshness, KEY_DRILL_COMMENT_PREFIX, KeyId, KeyRole,
    MAX_INSTALLER_LEN, MAX_MANIFEST_LEN, MAX_SIGNATURE_LEN, OfferKind, Purpose,
    TRUSTED_COMMENT_PREFIX, TrustAnchors, TrustState, UpdateRefusal, VerifiedManifest, VerifyInput,
    Version, apply_trust_report, tries_alternate, verify_manifest,
};
use support::{DAY, Draft, Key, Signed, T0, anchors, anchors_with_ids, sign_raw};

use KeyRole::{Backup, Primary};

/// The day after T0.
const NOW: u64 = T0 + DAY;

fn installed() -> Version {
    Version::new(0, 2, 0)
}

/// `Purpose::Check`, installed 0.2.0, x64, one day after T0, no tag.
fn check(
    signed: &Signed,
    anchors: &TrustAnchors,
    state: &TrustState,
) -> Result<VerifiedManifest, UpdateRefusal> {
    with(signed, anchors, state, |_| {})
}

fn with(
    signed: &Signed,
    anchors: &TrustAnchors,
    state: &TrustState,
    change: impl FnOnce(&mut VerifyInput<'_>),
) -> Result<VerifiedManifest, UpdateRefusal> {
    let installed = installed();
    let mut input = VerifyInput {
        manifest: &signed.manifest,
        signature: &signed.signature,
        anchors,
        state,
        installed: &installed,
        arch: Arch::X64,
        now_unix: NOW,
        tag: None,
        purpose: Purpose::Check,
    };
    change(&mut input);
    verify_manifest(&input)
}

fn empty() -> TrustState {
    TrustState::default()
}

fn malformed(result: Result<VerifiedManifest, UpdateRefusal>) -> bool {
    matches!(result, Err(UpdateRefusal::ManifestMalformed { .. }))
}

fn asset_malformed(result: Result<VerifiedManifest, UpdateRefusal>) -> bool {
    matches!(result, Err(UpdateRefusal::AssetMalformed { .. }))
}

// ---------------------------------------------------------------------------------------------
// Normal cases.

#[test]
fn primary_and_backup_signatures_verify() {
    let (p, b) = (Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[]);
    let signed = Draft::new("0.2.1", T0, &[&p]).signed_by(&p);
    let verified = check(&signed, &trust, &empty()).unwrap();
    assert_eq!(verified.version, Version::new(0, 2, 1));
    assert_eq!(verified.issued_at, T0);
    assert_eq!(verified.expires, T0 + 180 * DAY);
    assert_eq!(verified.signer, p.id);
    assert_eq!(verified.signer_role, Primary);
    assert_eq!(
        verified.signer_fingerprint,
        public_key_fingerprint(&p.public).unwrap()
    );
    assert!(!verified.signer_is_dev);
    assert_eq!(verified.key_ids, vec![p.id]);
    assert!(verified.revoked.is_empty());
    assert_eq!(verified.min_from_version, None);
    assert_eq!(verified.asset.arch, Arch::X64);
    assert_eq!(verified.asset.name, "MKLM-Setup-0.2.1-x64.exe");
    assert_eq!(verified.asset.size, 6_291_456);
    assert_eq!(verified.freshness, Freshness::Fresh);
    assert_eq!(verified.offer, OfferKind::Newer);

    let signed = Draft::new("0.2.1", T0, &[&b]).signed_by(&b);
    let verified = check(&signed, &trust, &empty()).unwrap();
    assert_eq!(verified.signer, b.id);
    assert_eq!(verified.signer_role, Backup);

    // The helper's purpose accepts a newer version.
    let verified = with(&signed, &trust, &empty(), |input| {
        input.purpose = Purpose::Install;
    })
    .unwrap();
    assert_eq!(verified.offer, OfferKind::Newer);
    // The other architecture.
    let verified = with(&signed, &trust, &empty(), |input| input.arch = Arch::Arm64).unwrap();
    assert_eq!(verified.asset.name, "MKLM-Setup-0.2.1-arm64.exe");
    assert_eq!(verified.asset.size, 6_029_312);
}

// ---------------------------------------------------------------------------------------------
// Tampering.

#[test]
fn every_changed_manifest_byte_is_a_bad_signature() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed = Draft::new("0.2.1", T0, &[&p]).signed_by(&p);
    for index in 0..signed.manifest.len() {
        let mut tampered = signed.clone();
        tampered.manifest[index] ^= 0x01;
        assert_eq!(
            check(&tampered, &trust, &empty()),
            Err(UpdateRefusal::BadSignature),
            "byte {index}"
        );
    }
    for extra in [&b" "[..], b"\n", b"\r\n", b"{}", b"\0"] {
        let mut tampered = signed.clone();
        tampered.manifest.extend_from_slice(extra);
        assert_eq!(
            check(&tampered, &trust, &empty()),
            Err(UpdateRefusal::BadSignature),
            "{extra:?}"
        );
    }
    let mut shorter = signed.clone();
    shorter.manifest.pop();
    assert_eq!(
        check(&shorter, &trust, &empty()),
        Err(UpdateRefusal::BadSignature)
    );
}

/// The lines of a signature file.
fn lines(signed: &Signed) -> Vec<String> {
    String::from_utf8(signed.signature.clone())
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn with_lines(signed: &Signed, lines: &[String]) -> Signed {
    Signed {
        manifest: signed.manifest.clone(),
        signature: format!("{}\n", lines.join("\n")).into_bytes(),
    }
}

/// Replaces the character at `index` of a base64 line by another base64 character.
fn change_char(line: &str, index: usize) -> String {
    let mut bytes = line.as_bytes().to_vec();
    bytes[index] = if bytes[index] == b'A' { b'B' } else { b'A' };
    String::from_utf8(bytes).unwrap()
}

#[test]
fn every_changed_signature_part_is_refused() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed = Draft::new("0.2.1", T0, &[&p]).signed_by(&p);
    let original = lines(&signed);
    assert_eq!(original.len(), 4);
    let refused = |lines: &[String]| check(&with_lines(&signed, lines), &trust, &empty());

    // The signature line past the algorithm and the key ID (base64 characters 14..).
    for index in 14..original[1].len() - 1 {
        let mut changed = original.clone();
        changed[1] = change_char(&original[1], index);
        let result = refused(&changed);
        assert!(
            matches!(
                result,
                Err(UpdateRefusal::BadSignature | UpdateRefusal::SignatureMalformed)
            ),
            "signature character {index}: {result:?}"
        );
    }
    // The global signature line.
    for index in 0..original[3].len() - 2 {
        let mut changed = original.clone();
        changed[3] = change_char(&original[3], index);
        let result = refused(&changed);
        assert!(
            matches!(
                result,
                Err(UpdateRefusal::BadSignature | UpdateRefusal::SignatureMalformed)
            ),
            "global signature character {index}: {result:?}"
        );
    }
    // The trusted comment, keeping its prefix: the global signature no longer verifies.
    for comment in [
        format!("trusted comment: {TRUSTED_COMMENT_PREFIX} version=9.9.9 issued_at={T0}"),
        format!("trusted comment: {TRUSTED_COMMENT_PREFIX}"),
        format!("{} ", original[2]),
    ] {
        let mut changed = original.clone();
        changed[2] = comment.clone();
        assert_eq!(
            refused(&changed),
            Err(UpdateRefusal::BadSignature),
            "{comment}"
        );
    }
    // Another prefix.
    for comment in [
        "trusted comment: mklm-latest-json v2 version=0.2.1".to_string(),
        format!("trusted comment: {TRUSTED_COMMENT_PREFIX}x"),
        format!("trusted comment:  {TRUSTED_COMMENT_PREFIX}"),
        "trusted comment: timestamp:1792022400\tfile:latest.json\thashed".to_string(),
    ] {
        let mut changed = original.clone();
        changed[2] = comment.clone();
        assert_eq!(
            refused(&changed),
            Err(UpdateRefusal::WrongTrustedComment),
            "{comment}"
        );
    }
    // A legacy signature (algorithm `Ed`): `ED…` and `Ed…` differ in the second base64
    // character only ("RU" / "RW").
    assert!(original[1].starts_with("RU"), "{}", original[1]);
    let mut legacy = original.clone();
    legacy[1] = format!("RW{}", &original[1][2..]);
    assert_eq!(refused(&legacy), Err(UpdateRefusal::SignatureMalformed));
    // Lines missing, added, not UTF-8, not base64.
    assert_eq!(
        refused(&original[..3]),
        Err(UpdateRefusal::SignatureMalformed)
    );
    let mut added = original.clone();
    added.push("extra".to_string());
    assert_eq!(refused(&added), Err(UpdateRefusal::SignatureMalformed));
    let mut not_base64 = original.clone();
    not_base64[1].insert(5, ' ');
    assert_eq!(refused(&not_base64), Err(UpdateRefusal::SignatureMalformed));
    let binary = Signed {
        manifest: signed.manifest.clone(),
        signature: vec![0xFF, 0xFE, b'x'],
    };
    assert_eq!(
        check(&binary, &trust, &empty()),
        Err(UpdateRefusal::SignatureMalformed)
    );
    // CRLF line breaks are the same file.
    let crlf = Signed {
        manifest: signed.manifest.clone(),
        signature: format!("{}\r\n", original.join("\r\n")).into_bytes(),
    };
    assert!(check(&crlf, &trust, &empty()).is_ok());
}

#[test]
fn trusted_comments_keep_their_purposes_apart() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let draft = Draft::new("0.2.1", T0, &[&p]);
    for comment in [
        format!("{DEV_TRUSTED_COMMENT_PREFIX} version=0.2.1 issued_at={T0}"),
        KEY_DRILL_COMMENT_PREFIX.to_string(),
        String::new(),
    ] {
        let manifest = draft.bytes();
        let signed = Signed {
            signature: p.sign(&manifest, &comment),
            manifest,
        };
        assert_eq!(
            check(&signed, &trust, &empty()),
            Err(UpdateRefusal::WrongTrustedComment),
            "{comment:?}"
        );
    }
    // The bare prefix is enough.
    let manifest = draft.bytes();
    let signed = Signed {
        signature: p.sign(&manifest, TRUSTED_COMMENT_PREFIX),
        manifest,
    };
    assert!(check(&signed, &trust, &empty()).is_ok());
}

#[cfg(all(debug_assertions, mklm_update_dev))]
#[test]
fn the_development_key_signs_rehearsals_only_and_records_nothing() {
    let (p, dev) = (Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p)], &[])
        .with_dev_key(&dev.public)
        .unwrap();
    let draft = Draft::new("0.2.1", T0, &[&dev]).revoking(&[p.id]);
    let manifest = draft.bytes();
    let production = Signed {
        signature: dev.sign(&manifest, &draft.comment()),
        manifest: manifest.clone(),
    };
    assert_eq!(
        check(&production, &trust, &empty()),
        Err(UpdateRefusal::WrongTrustedComment)
    );
    let rehearsal = Signed {
        signature: dev.sign(
            &manifest,
            &format!("{DEV_TRUSTED_COMMENT_PREFIX} version=0.2.1 issued_at={T0}"),
        ),
        manifest,
    };
    let verified = check(&rehearsal, &trust, &empty()).unwrap();
    assert!(verified.signer_is_dev);
    // Its revocations are ignored and nothing is recorded.
    assert!(verified.revoked.is_empty());
    let state = TrustState {
        max_issued_at: [(p.id.to_text(), T0 - DAY)].into(),
        ..TrustState::default()
    };
    assert_eq!(state.recorded(&verified, NOW), state);
    // So the rehearsal cannot show a rollback: after this manifest, an older rehearsal manifest
    // still passes (design m5b B.3: prepare-release --dev --issued-at is for the expiry display;
    // BUILD-RUN-1).
    let after = empty().recorded(&verified, NOW);
    assert_eq!(after, empty());
    let older_at = T0 - 7 * DAY;
    let older = Draft::new("0.2.1", older_at, &[&dev]).bytes();
    let older = Signed {
        signature: dev.sign(
            &older,
            &format!("{DEV_TRUSTED_COMMENT_PREFIX} version=0.2.1 issued_at={older_at}"),
        ),
        manifest: older,
    };
    assert_eq!(check(&older, &trust, &after).unwrap().issued_at, older_at);
    assert_eq!(
        apply_trust_report(
            &rehearsal.manifest,
            &rehearsal.signature,
            &trust,
            &installed(),
            Arch::X64,
            &state,
            NOW
        ),
        Ok(None)
    );
    // The production key cannot sign a rehearsal.
    let draft = Draft::new("0.2.1", T0, &[&p]);
    let manifest = draft.bytes();
    let signed = Signed {
        signature: p.sign(&manifest, DEV_TRUSTED_COMMENT_PREFIX),
        manifest,
    };
    assert_eq!(
        check(&signed, &trust, &empty()),
        Err(UpdateRefusal::WrongTrustedComment)
    );
}

// ---------------------------------------------------------------------------------------------
// Keys.

#[test]
fn unknown_keys_and_borrowed_key_ids() {
    let (p, stranger) = (Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed = Draft::new("0.2.1", T0, &[&stranger]).signed_by(&stranger);
    assert_eq!(
        check(&signed, &trust, &empty()),
        Err(UpdateRefusal::UnknownKey {
            key_id: stranger.id.to_text()
        })
    );
    // The stranger's signature with the trusted key's ID written into it: the key is found, the
    // signature does not verify with it.
    let original = lines(&signed);
    let mut bytes = base64_decode(&original[1]);
    bytes[2..10].copy_from_slice(&p.id.0);
    let mut changed = original.clone();
    changed[1] = base64_encode(&bytes);
    let borrowed = with_lines(&signed, &changed);
    // The manifest lists the stranger; the ID check comes later, the signature fails first.
    assert_eq!(
        check(&borrowed, &trust, &empty()),
        Err(UpdateRefusal::BadSignature)
    );
}

#[test]
fn key_ids_of_the_manifest() {
    let (p, b, other) = (Key::new(), Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[]);
    let with_ids = |ids: Vec<String>| {
        let mut draft = Draft::new("0.2.1", T0, &[&p]);
        draft.manifest.key_ids = ids;
        draft.signed_by(&p)
    };
    for ids in [
        vec![],
        vec![p.id.to_text(), b.id.to_text(), other.id.to_text()],
        vec![p.id.to_text(), p.id.to_text()],
        vec![p.id.to_text().to_lowercase()],
        vec![format!("{}0", p.id)],
    ] {
        assert!(
            malformed(check(&with_ids(ids.clone()), &trust, &empty())),
            "{ids:?}"
        );
    }
    assert_eq!(
        check(&with_ids(vec![b.id.to_text()]), &trust, &empty()),
        Err(UpdateRefusal::SignerNotListed {
            key_id: p.id.to_text()
        })
    );
    // An unknown ID next to the signer (a key transition) is fine, in either order.
    let verified = check(
        &with_ids(vec![other.id.to_text(), p.id.to_text()]),
        &trust,
        &empty(),
    )
    .unwrap();
    assert_eq!(verified.key_ids, vec![other.id, p.id]);
}

// ---------------------------------------------------------------------------------------------
// Revocations (SECURITY-3, FIX-VERIFICATION-1).

#[test]
fn revoked_keys_are_refused() {
    let (p, b) = (Key::new(), Key::new());
    let signed = Draft::new("0.2.1", T0, &[&p]).signed_by(&p);
    // By the build.
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[&p]);
    assert_eq!(
        check(&signed, &trust, &empty()),
        Err(UpdateRefusal::RevokedKey {
            key_id: p.id.to_text()
        })
    );
    // By the record: ID and fingerprint.
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[]);
    let revoked_by_b = Draft::new("0.2.1", T0, &[&b])
        .revoking(&[p.id])
        .signed_by(&b);
    let state = empty().recorded(&check(&revoked_by_b, &trust, &empty()).unwrap(), NOW);
    assert!(state.is_revoked(p.id, public_key_fingerprint(&p.public).unwrap()));
    assert_eq!(
        check(&signed, &trust, &state),
        Err(UpdateRefusal::RevokedKey {
            key_id: p.id.to_text()
        })
    );
    // A recorded revocation of the same ID with another fingerprint does not hit this key.
    let mut other_fingerprint = state.clone();
    other_fingerprint.revoked = other_fingerprint
        .revoked
        .into_iter()
        .map(|mut revoked| {
            revoked.fingerprint = "00".repeat(32);
            revoked
        })
        .collect();
    assert!(check(&signed, &trust, &other_fingerprint).is_ok());
}

#[test]
fn the_revocation_table_of_design_b2() {
    let (p, p_other, b, unknown) = (Key::new(), Key::new(), Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p), (Primary, &p_other), (Backup, &b)], &[]);

    // A manifest may not revoke its own key.
    for signer in [&p, &b] {
        let signed = Draft::new("0.2.1", T0, &[signer])
            .revoking(&[signer.id])
            .signed_by(signer);
        assert_eq!(
            check(&signed, &trust, &empty()),
            Err(UpdateRefusal::IllegalRevocation {
                key_id: signer.id.to_text()
            })
        );
    }
    // The primary key revokes the backup key: accepted, nothing recorded, and backup-signed
    // manifests still verify afterwards (FIX-VERIFICATION-1).
    let signed = Draft::new("0.2.1", T0, &[&p])
        .revoking(&[b.id])
        .signed_by(&p);
    let verified = check(&signed, &trust, &empty()).unwrap();
    assert!(verified.revoked.is_empty());
    let state = empty().recorded(&verified, NOW);
    assert!(state.revoked.is_empty());
    let by_backup = Draft::new("0.2.2", T0 + 10, &[&b]).signed_by(&b);
    assert!(check(&by_backup, &trust, &state).is_ok());
    // Another embedded primary key, by the primary or by the backup key: recorded.
    for signer in [&p, &b] {
        let signed = Draft::new("0.2.1", T0, &[signer])
            .revoking(&[p_other.id])
            .signed_by(signer);
        let verified = check(&signed, &trust, &empty()).unwrap();
        assert_eq!(
            verified.revoked.keys().copied().collect::<Vec<_>>(),
            vec![p_other.id]
        );
    }
    // The backup key revokes the primary key: recorded; the primary's manifests then fail.
    let signed = Draft::new("0.2.1", T0, &[&b])
        .revoking(&[p.id])
        .signed_by(&b);
    let state = empty().recorded(&check(&signed, &trust, &empty()).unwrap(), NOW);
    let by_primary = Draft::new("0.2.2", T0 + 10, &[&p]).signed_by(&p);
    assert_eq!(
        check(&by_primary, &trust, &state),
        Err(UpdateRefusal::RevokedKey {
            key_id: p.id.to_text()
        })
    );
    // An ID this build does not embed: ignored, not recorded.
    let signed = Draft::new("0.2.1", T0, &[&p])
        .revoking(&[unknown.id])
        .signed_by(&p);
    let verified = check(&signed, &trust, &empty()).unwrap();
    assert!(verified.revoked.is_empty());
    assert!(empty().recorded(&verified, NOW).revoked.is_empty());
    // An ID the build already revokes: nothing happens.
    let trust_revoking = anchors(&[(Primary, &p), (Backup, &b)], &[&p_other]);
    let signed = Draft::new("0.2.1", T0, &[&p])
        .revoking(&[p_other.id])
        .signed_by(&p);
    assert!(
        check(&signed, &trust_revoking, &empty())
            .unwrap()
            .revoked
            .is_empty()
    );
    // Malformed IDs.
    let mut draft = Draft::new("0.2.1", T0, &[&p]);
    draft.manifest.revoked_keys = vec!["nope".to_string()];
    assert!(malformed(check(&draft.signed_by(&p), &trust, &empty())));
}

// ---------------------------------------------------------------------------------------------
// Anti-rollback (SECURITY-5, SECURITY-12, RELIABILITY-6).

#[test]
fn rollback_threshold_over_all_keys() {
    let (p, b) = (Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[]);
    let state = empty().recorded(
        &check(
            &Draft::new("0.2.1", T0, &[&p]).signed_by(&p),
            &trust,
            &empty(),
        )
        .unwrap(),
        NOW,
    );
    assert_eq!(state.max_issued_at(p.id), Some(T0));
    // Older: refused; equal: accepted (the same manifest fetched again).
    let older = Draft::new("0.2.1", T0 - 1, &[&p]).signed_by(&p);
    assert_eq!(
        check(&older, &trust, &state),
        Err(UpdateRefusal::Rollback {
            issued_at: T0 - 1,
            seen: T0
        })
    );
    assert!(
        check(
            &Draft::new("0.2.1", T0, &[&p]).signed_by(&p),
            &trust,
            &state
        )
        .is_ok()
    );
    // Across keys: key A's value refuses key B's older manifest.
    let older_by_backup = Draft::new("0.2.1", T0 - 1, &[&b]).signed_by(&b);
    assert_eq!(
        check(&older_by_backup, &trust, &state),
        Err(UpdateRefusal::Rollback {
            issued_at: T0 - 1,
            seen: T0
        })
    );
    // A revoked key's value no longer counts (by the build here).
    let trust_revoking_p = anchors(&[(Backup, &b)], &[&p]);
    assert!(check(&older_by_backup, &trust_revoking_p, &state).is_ok());
    assert_eq!(
        state.rollback_threshold(&trust_revoking_p, &BTreeSet::new()),
        None
    );
    assert_eq!(state.rollback_threshold(&trust, &BTreeSet::new()), Some(T0));
    assert_eq!(
        state.rollback_threshold(&trust, &BTreeSet::from([p.id])),
        None
    );
    // ...or by the record (ID and fingerprint of a key the build knows).
    let revocation = Draft::new("0.2.1", T0, &[&b])
        .revoking(&[p.id])
        .signed_by(&b);
    let revoked_p = state.recorded(&check(&revocation, &trust, &state).unwrap(), NOW);
    assert_eq!(revoked_p.max_issued_at(p.id), Some(T0));
    assert_eq!(revoked_p.max_issued_at(b.id), Some(T0));
    let mut only_p = revoked_p.clone();
    only_p.max_issued_at.remove(&b.id.to_text());
    assert_eq!(only_p.rollback_threshold(&trust, &BTreeSet::new()), None);
    // A revocation recorded for another key under the same ID does not exclude P's value.
    let mut other_key = only_p.clone();
    other_key.revoked = other_key
        .revoked
        .into_iter()
        .map(|mut revoked| {
            revoked.fingerprint = "00".repeat(32);
            revoked
        })
        .collect();
    assert_eq!(
        other_key.rollback_threshold(&trust, &BTreeSet::new()),
        Some(T0)
    );
    // A key this build does not know is excluded by its ID alone.
    let stranger = Key::new();
    let mut unknown = TrustState::default();
    unknown.max_issued_at.insert(stranger.id.to_text(), T0 + 5);
    assert_eq!(
        unknown.rollback_threshold(&trust, &BTreeSet::new()),
        Some(T0 + 5)
    );
    unknown.revoked.insert(mklm_update::RevokedKey {
        key_id: stranger.id.to_text(),
        fingerprint: "11".repeat(32),
    });
    assert_eq!(unknown.rollback_threshold(&trust, &BTreeSet::new()), None);
}

#[test]
fn a_leaked_key_cannot_block_its_own_revocation() {
    let (p, b) = (Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[]);
    // A manifest from the leaked primary key, dated far in the future: recorded at `NOW` at most.
    let future = Draft::new("9.0.0", T0 + 700 * DAY, &[&p]).signed_by(&p);
    let state = empty().recorded(&check(&future, &trust, &empty()).unwrap(), NOW);
    assert_eq!(state.max_issued_at(p.id), Some(NOW));
    // The backup key's revocation of P, dated before NOW, passes the rollback check because the
    // manifest itself revokes P.
    let revocation = Draft::new("0.2.1", NOW - 10, &[&b])
        .revoking(&[p.id])
        .signed_by(&b);
    let verified = check(&revocation, &trust, &state).unwrap();
    let state = state.recorded(&verified, NOW + 5);
    assert!(state.is_revoked(p.id, public_key_fingerprint(&p.public).unwrap()));
    // A correctly dated next release passes: the record never exceeded the time of recording.
    let next = Draft::new("0.2.2", NOW + DAY, &[&b]).signed_by(&b);
    assert!(check(&next, &trust, &state).is_ok());
    // Without the revocation, the future-dated record still lets later correct manifests through.
    let state = empty().recorded(&check(&future, &trust, &empty()).unwrap(), NOW);
    let correct = Draft::new("0.2.2", NOW + 60, &[&p]).signed_by(&p);
    assert!(check(&correct, &trust, &state).is_ok());
}

// ---------------------------------------------------------------------------------------------
// Key transitions (SECURITY-4, OPS-UX-TEST-1) and the backup key rotation (FIX-VERIFICATION-1).

fn with_alternate(
    main: &Signed,
    alternate: Option<&Signed>,
    trust: &TrustAnchors,
    state: &TrustState,
) -> Result<VerifiedManifest, UpdateRefusal> {
    match check(main, trust, state) {
        Err(error) if tries_alternate(&error) => match alternate {
            Some(alternate) => check(alternate, trust, state).map_err(|_| error),
            None => Err(error),
        },
        result => result,
    }
}

#[test]
fn a_primary_key_transition() {
    let (p1, b1, p2) = (Key::new(), Key::new(), Key::new());
    let old = anchors(&[(Primary, &p1), (Backup, &b1)], &[]);
    let new = anchors(&[(Primary, &p2), (Backup, &b1)], &[&p1]);

    // (a) N: main B1, alternate P2, revoking P1. The old build takes the main signature and
    // records P1's revocation.
    let n = Draft::new("0.3.0", T0, &[&b1, &p2]).revoking(&[p1.id]);
    let n_main = n.signed_by(&b1);
    let n_alt = n.signed_by(&p2);
    let verified = with_alternate(&n_main, Some(&n_alt), &old, &empty()).unwrap();
    assert_eq!(verified.signer, b1.id);
    let old_state = empty().recorded(&verified, NOW);
    assert!(old_state.is_revoked(p1.id, public_key_fingerprint(&p1.public).unwrap()));

    // (b) An old build that missed N gets N+1 (main P2, alternate B1): the main signature is
    // `UnknownKey`, which tries the alternate; it verifies and records P1's revocation.
    let n1 = Draft::new("0.3.1", T0 + 30 * DAY, &[&p2, &b1]).revoking(&[p1.id]);
    let n1_main = n1.signed_by(&p2);
    let n1_alt = n1.signed_by(&b1);
    let main_error = check(&n1_main, &old, &empty()).unwrap_err();
    assert_eq!(
        main_error,
        UpdateRefusal::UnknownKey {
            key_id: p2.id.to_text()
        }
    );
    assert!(tries_alternate(&main_error));
    let verified = with_alternate(&n1_main, Some(&n1_alt), &old, &empty()).unwrap();
    assert_eq!(verified.signer, b1.id);
    assert_eq!(
        verified.revoked.keys().copied().collect::<Vec<_>>(),
        vec![p1.id]
    );

    // (c) The new build takes N+1's main signature.
    assert_eq!(check(&n1_main, &new, &empty()).unwrap().signer, p2.id);

    // (d) The old build that recorded P1's revocation refuses a forgery by P1 (`RevokedKey`),
    // and without an alternate signature that stays the answer.
    let forged = Draft::new("6.6.6", T0 + 40 * DAY, &[&p1]).signed_by(&p1);
    let error = with_alternate(&forged, None, &old, &old_state).unwrap_err();
    assert_eq!(
        error,
        UpdateRefusal::RevokedKey {
            key_id: p1.id.to_text()
        }
    );
    assert!(tries_alternate(&error));

    // (e) Other failures never try the alternate.
    let mut tampered = n1_main.clone();
    tampered.manifest[3] ^= 1;
    let error = check(&tampered, &new, &empty()).unwrap_err();
    assert_eq!(error, UpdateRefusal::BadSignature);
    assert!(!tries_alternate(&error));
}

#[test]
fn the_backup_key_rotation() {
    let (p1, b1, p2, b2) = (Key::new(), Key::new(), Key::new(), Key::new());
    let v_p1_b1 = anchors(&[(Primary, &p1), (Backup, &b1)], &[]);
    let v_p1_b2 = anchors(&[(Primary, &p1), (Backup, &b2)], &[&b1]);
    let v_p2_b1 = anchors(&[(Primary, &p2), (Backup, &b1)], &[&p1]);
    let v_p2_b2 = anchors(&[(Primary, &p2), (Backup, &b2)], &[&p1, &b1]);

    // (a) After B1 leaked: N (main P1, revoking B1). Both builds with P1 accept it.
    let n = Draft::new("0.3.0", T0, &[&p1])
        .revoking(&[b1.id])
        .signed_by(&p1);
    let verified = check(&n, &v_p1_b1, &empty()).unwrap();
    assert!(verified.revoked.is_empty());
    let state = empty().recorded(&verified, NOW);
    assert!(state.revoked.is_empty());
    // B1's manifests still verify in that build (it records nothing: rule 4).
    let by_b1 = Draft::new("0.3.1", T0 + 10, &[&b1]).signed_by(&b1);
    assert!(check(&by_b1, &v_p1_b1, &state).is_ok());
    let verified = check(&n, &v_p1_b2, &empty()).unwrap();
    assert!(verified.revoked.is_empty());

    // (b) After the window: the rotation release (main P2, revoking P1 and B1).
    let rotation = Draft::new("0.4.0", T0 + 500 * DAY, &[&p2])
        .revoking(&[p1.id, b1.id])
        .signed_by(&p2);
    let verified = check(&rotation, &v_p2_b1, &empty()).unwrap();
    assert!(verified.revoked.is_empty(), "{:?}", verified.revoked);
    assert!(
        check(&rotation, &v_p2_b2, &empty())
            .unwrap()
            .revoked
            .is_empty()
    );

    // (c) The (P1, B1) build does not know P2: outside the window, as intended.
    assert_eq!(
        check(&rotation, &v_p1_b1, &empty()),
        Err(UpdateRefusal::UnknownKey {
            key_id: p2.id.to_text()
        })
    );
}

// ---------------------------------------------------------------------------------------------
// RecordTrust (SECURITY-5, FIX-VERIFICATION-6).

#[test]
fn trust_reports() {
    let (p, b) = (Key::new(), Key::new());
    let trust = anchors(&[(Primary, &p), (Backup, &b)], &[]);
    let report = |signed: &Signed, machine: &TrustState| {
        apply_trust_report(
            &signed.manifest,
            &signed.signature,
            &trust,
            &installed(),
            Arch::X64,
            machine,
            NOW,
        )
    };
    let machine = empty();
    // (a) A newer manifest: merged record.
    let newer = Draft::new("0.2.1", T0, &[&b])
        .revoking(&[p.id])
        .signed_by(&b);
    let merged = report(&newer, &machine).unwrap().unwrap();
    assert_eq!(merged.max_issued_at(b.id), Some(T0));
    assert!(merged.is_revoked(p.id, public_key_fingerprint(&p.public).unwrap()));
    // The value recorded is min(issued_at, now).
    let future = Draft::new("0.2.1", NOW + 50, &[&b]).signed_by(&b);
    assert_eq!(
        report(&future, &machine)
            .unwrap()
            .unwrap()
            .max_issued_at(b.id),
        Some(NOW)
    );
    // (b) The same manifest again: nothing new.
    assert_eq!(report(&newer, &merged), Ok(None));
    // (c) A changed signature: refused, nothing to write.
    let mut bad = newer.clone();
    bad.manifest[10] ^= 1;
    assert_eq!(report(&bad, &merged), Err(UpdateRefusal::BadSignature));
    // (d) Older than the machine record: refused.
    let older = Draft::new("0.2.0", T0 - DAY, &[&b]).signed_by(&b);
    assert_eq!(
        report(&older, &merged),
        Err(UpdateRefusal::Rollback {
            issued_at: T0 - DAY,
            seen: T0
        })
    );
    // (f) A version older than the installed one still advances the record (Check).
    let old_version = Draft::new("0.1.9", T0 + 5, &[&b]).signed_by(&b);
    let advanced = report(&old_version, &merged).unwrap().unwrap();
    assert_eq!(advanced.max_issued_at(b.id), Some(T0 + 5));
}

// ---------------------------------------------------------------------------------------------
// Content rules.

#[test]
fn expiry_is_advisory() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed = Draft::new("0.2.1", T0, &[&p]).signed_by(&p);
    for purpose in [Purpose::Check, Purpose::Install] {
        let verified = with(&signed, &trust, &empty(), |input| {
            input.now_unix = T0 + 181 * DAY;
            input.purpose = purpose;
        })
        .unwrap();
        assert_eq!(verified.freshness, Freshness::Expired);
    }
    let at_expiry = with(&signed, &trust, &empty(), |input| {
        input.now_unix = T0 + 180 * DAY;
    })
    .unwrap();
    assert_eq!(at_expiry.freshness, Freshness::Fresh);
    let timestamps = |issued_at: u64, expires: u64| {
        let mut draft = Draft::new("0.2.1", issued_at, &[&p]);
        draft.manifest.expires = expires;
        check(&draft.signed_by(&p), &trust, &empty())
    };
    assert_eq!(timestamps(T0, T0), Err(UpdateRefusal::BadTimestamps));
    assert_eq!(timestamps(T0, T0 - 1), Err(UpdateRefusal::BadTimestamps));
    assert_eq!(
        timestamps(T0, T0 + 800 * DAY + 1),
        Err(UpdateRefusal::BadTimestamps)
    );
    assert!(timestamps(T0, T0 + 800 * DAY).is_ok());
    assert!(timestamps(T0, T0 + 1).is_ok());
}

#[test]
fn assets_of_both_architectures() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed_with = |change: &dyn Fn(&mut Draft)| {
        let mut draft = Draft::new("0.2.1", T0, &[&p]);
        change(&mut draft);
        check(&draft.signed_by(&p), &trust, &empty())
    };
    type Change = Box<dyn Fn(&mut Draft)>;
    let cases: Vec<(&str, Change)> = vec![
        (
            "one asset",
            Box::new(|d| {
                d.manifest.assets.pop();
            }),
        ),
        (
            "three assets",
            Box::new(|d| {
                let extra = d.manifest.assets[0].clone();
                d.manifest.assets.push(extra);
            }),
        ),
        (
            "the same arch twice",
            Box::new(|d| {
                d.manifest.assets[1] = d.manifest.assets[0].clone();
            }),
        ),
        ("no assets", Box::new(|d| d.manifest.assets.clear())),
        (
            "another version",
            Box::new(|d| {
                d.manifest.assets[0].name = "MKLM-Setup-0.2.0-x64.exe".to_string();
            }),
        ),
        (
            "another arch in the name",
            Box::new(|d| {
                d.manifest.assets[0].name = "MKLM-Setup-0.2.1-arm64.exe".to_string();
            }),
        ),
        (
            "upper case",
            Box::new(|d| {
                d.manifest.assets[0].name = "MKLM-Setup-0.2.1-X64.exe".to_string();
            }),
        ),
        (
            "a path",
            Box::new(|d| {
                d.manifest.assets[0].name = "..\\MKLM-Setup-0.2.1-x64.exe".to_string();
            }),
        ),
        (
            "a slash",
            Box::new(|d| {
                d.manifest.assets[1].name = "x/MKLM-Setup-0.2.1-arm64.exe".to_string();
            }),
        ),
        ("size 0", Box::new(|d| d.manifest.assets[0].size = 0)),
        (
            "size over 64 MiB",
            Box::new(|d| d.manifest.assets[1].size = MAX_INSTALLER_LEN + 1),
        ),
        (
            "63 hex digits",
            Box::new(|d| {
                d.manifest.assets[0].sha256.pop();
            }),
        ),
        (
            "upper-case hex",
            Box::new(|d| {
                d.manifest.assets[0].sha256 = d.manifest.assets[0].sha256.to_uppercase();
            }),
        ),
        (
            "not hex",
            Box::new(|d| {
                d.manifest.assets[0].sha256 = "z".repeat(64);
            }),
        ),
    ];
    for (name, change) in &cases {
        assert!(asset_malformed(signed_with(change.as_ref())), "{name}");
    }
    // Exactly 64 MiB is allowed; the assets may come in any order.
    assert!(signed_with(&|d| d.manifest.assets[0].size = MAX_INSTALLER_LEN).is_ok());
    let reversed = signed_with(&|d| d.manifest.assets.reverse()).unwrap();
    assert_eq!(reversed.asset.arch, Arch::X64);
}

#[test]
fn size_limits() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let draft = Draft::new("0.2.1", T0, &[&p]);
    let mut manifest = draft.bytes();
    manifest.resize(MAX_MANIFEST_LEN + 1, b' ');
    let large = Signed {
        signature: p.sign(&manifest, &draft.comment()),
        manifest,
    };
    assert_eq!(
        check(&large, &trust, &empty()),
        Err(UpdateRefusal::ManifestTooLarge {
            len: MAX_MANIFEST_LEN as u64 + 1
        })
    );
    // Exactly the limit, padded with whitespace, is fine.
    let mut manifest = draft.bytes();
    manifest.resize(MAX_MANIFEST_LEN, b' ');
    let at_limit = Signed {
        signature: p.sign(&manifest, &draft.comment()),
        manifest,
    };
    assert!(check(&at_limit, &trust, &empty()).is_ok());
    let mut signed = draft.signed_by(&p);
    signed.signature.resize(MAX_SIGNATURE_LEN + 1, b'\n');
    assert_eq!(
        check(&signed, &trust, &empty()),
        Err(UpdateRefusal::SignatureTooLarge {
            len: MAX_SIGNATURE_LEN as u64 + 1
        })
    );
}

#[test]
fn schema_product_channel_and_strict_json() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed = |change: &dyn Fn(&mut Draft)| {
        let mut draft = Draft::new("0.2.1", T0, &[&p]);
        change(&mut draft);
        check(&draft.signed_by(&p), &trust, &empty())
    };
    assert_eq!(
        signed(&|d| d.manifest.schema = 2),
        Err(UpdateRefusal::UnsupportedSchema { schema: 2 })
    );
    assert_eq!(
        signed(&|d| d.manifest.product = "mklm".to_string()),
        Err(UpdateRefusal::WrongProduct)
    );
    assert_eq!(
        signed(&|d| d.manifest.channel = "beta".to_string()),
        Err(UpdateRefusal::WrongChannel)
    );
    let text = Draft::new("0.2.1", T0, &[&p]).manifest.to_canonical_json();
    let mut raw_cases: Vec<Vec<u8>> = vec![
        text.replacen(
            "\"schema\": 1,",
            "\"schema\": 1, \"url\": \"https://x\",",
            1,
        )
        .into_bytes(),
        text.replacen("\"schema\": 1,", "\"schema\": 1, \"schema\": 1,", 1)
            .into_bytes(),
        format!("{text}[]").into_bytes(),
        text.replacen("\"schema\": 1,", "\"schema\": \"1\",", 1)
            .into_bytes(),
        text.replacen("\"size\": 6291456,", "\"size\": \"6291456\",", 1)
            .into_bytes(),
    ];
    let mut bom = b"\xEF\xBB\xBF".to_vec();
    bom.extend_from_slice(text.as_bytes());
    raw_cases.push(bom);
    let mut latin1 = text.clone().into_bytes();
    latin1.push(0xE9);
    raw_cases.push(latin1);
    for raw in &raw_cases {
        // Signed correctly: the refusal is the parser's, after the signature.
        assert!(
            malformed(check(&sign_raw(&p, raw), &trust, &empty())),
            "{}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn versions_and_offers() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    for bad in [
        "v0.2.1",
        "0.2",
        "0.2.1-beta",
        "0.2.1+x",
        "00.2.1",
        "65536.0.0",
    ] {
        let mut draft = Draft::new("0.2.1", T0, &[&p]);
        draft.manifest.version = bad.to_string();
        assert_eq!(
            check(&draft.signed_by(&p), &trust, &empty()),
            Err(UpdateRefusal::BadVersion {
                text: bad.to_string()
            }),
            "{bad}"
        );
        let mut draft = Draft::new("0.2.1", T0, &[&p]);
        draft.manifest.min_from_version = Some(bad.to_string());
        assert_eq!(
            check(&draft.signed_by(&p), &trust, &empty()),
            Err(UpdateRefusal::BadVersion {
                text: bad.to_string()
            }),
            "min_from_version {bad}"
        );
    }
    let offer = |version: &str, installed: &str, purpose: Purpose| {
        let signed = Draft::new(version, T0, &[&p]).signed_by(&p);
        let installed = mklm_update::version::parse_installed_version(installed).unwrap();
        let input = VerifyInput {
            manifest: &signed.manifest,
            signature: &signed.signature,
            anchors: &trust,
            state: &TrustState::default(),
            installed: &installed,
            arch: Arch::X64,
            now_unix: NOW,
            tag: None,
            purpose,
        };
        verify_manifest(&input)
    };
    // The same version and older ones.
    assert_eq!(
        offer("0.2.0", "0.2.0", Purpose::Check).unwrap().offer,
        OfferKind::UpToDate
    );
    assert_eq!(
        offer("0.1.9", "0.2.0", Purpose::Check).unwrap().offer,
        OfferKind::UpToDate
    );
    for version in ["0.2.0", "0.1.9"] {
        assert_eq!(
            offer(version, "0.2.0", Purpose::Install),
            Err(UpdateRefusal::NotNewer {
                offered: version.to_string(),
                installed: "0.2.0".to_string()
            })
        );
    }
    // A development pre-release is older than its release.
    assert_eq!(
        offer("0.2.0", "0.2.0-dev.1", Purpose::Install)
            .unwrap()
            .offer,
        OfferKind::Newer
    );
    assert_eq!(
        offer("0.1.9", "0.2.0-dev.1", Purpose::Check).unwrap().offer,
        OfferKind::UpToDate
    );
    // min_from_version.
    let mut draft = Draft::new("0.3.0", T0, &[&p]);
    draft.manifest.min_from_version = Some("0.2.5".to_string());
    let signed = draft.signed_by(&p);
    let verified = check(&signed, &trust, &empty()).unwrap();
    assert_eq!(verified.offer, OfferKind::ManualRequired);
    assert_eq!(verified.min_from_version, Some(Version::new(0, 2, 5)));
    assert_eq!(
        with(&signed, &trust, &empty(), |input| input.purpose =
            Purpose::Install),
        Err(UpdateRefusal::ManualUpdateRequired {
            min_from: "0.2.5".to_string(),
            installed: "0.2.0".to_string()
        })
    );
    let at_min = Version::new(0, 2, 5);
    let state = TrustState::default();
    let verified = verify_manifest(&VerifyInput {
        manifest: &signed.manifest,
        signature: &signed.signature,
        anchors: &trust,
        state: &state,
        installed: &at_min,
        arch: Arch::X64,
        now_unix: NOW,
        tag: None,
        purpose: Purpose::Install,
    });
    assert_eq!(verified.unwrap().offer, OfferKind::Newer);
}

#[test]
fn the_tag_must_name_the_version() {
    let p = Key::new();
    let trust = anchors(&[(Primary, &p)], &[]);
    let signed = Draft::new("0.2.1", T0, &[&p]).signed_by(&p);
    assert!(
        with(&signed, &trust, &empty(), |input| input.tag =
            Some("v0.2.1"))
        .is_ok()
    );
    for tag in ["v0.2.0", "0.2.1", "v0.2.1-rc.1", ""] {
        assert_eq!(
            with(&signed, &trust, &empty(), |input| input.tag = Some(tag)),
            Err(UpdateRefusal::TagMismatch {
                tag: tag.to_string(),
                version: "0.2.1".to_string()
            }),
            "{tag}"
        );
    }
}

#[test]
fn records_round_trip() {
    let (p, b) = (Key::new(), Key::new());
    let trust = anchors_with_ids(&[(Primary, &p), (Backup, &b)], &[KeyId([1; 8])]);
    let signed = Draft::new("0.2.1", T0, &[&b])
        .revoking(&[p.id])
        .signed_by(&b);
    let state = empty().recorded(&check(&signed, &trust, &empty()).unwrap(), NOW);
    assert_eq!(TrustState::parse(&state.to_json()), Ok(state.clone()));
    assert!(state.is_ahead_of(&empty()));
    assert!(!empty().is_ahead_of(&state));
    assert_eq!(state.merged(&empty()), state);
}

// ---------------------------------------------------------------------------------------------
// Test data helpers.

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a' + 26),
            b'0'..=b'9' => u32::from(c - b'0' + 52),
            b'+' => 62,
            b'/' => 63,
            _ => 0,
        }
    };
    let mut out = Vec::new();
    for quad in text.as_bytes().chunks(4) {
        let pad = quad.iter().filter(|&&c| c == b'=').count();
        let word = quad.iter().fold(0u32, |acc, &c| {
            (acc << 6) | if c == b'=' { 0 } else { value(c) }
        });
        let bytes = word.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - pad]);
    }
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((word >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn test_base64_helpers_round_trip() {
    for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob"] {
        assert_eq!(base64_decode(&base64_encode(bytes)), bytes);
    }
}
