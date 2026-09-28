//! The update check and the download over the real verification and fetch policy (design m5b
//! F.4): a release played by a fake transport, manifests signed with throwaway keys, the user's
//! cache in a scratch folder. No network, no registry.

mod update_common;

use std::sync::atomic::AtomicBool;

use mklm_client::update::cache::{ErrorClass, UpdateCache};
use mklm_client::update::check::{CheckError, CheckOutcome, check, reverify_cached};
use mklm_client::update::download::download;
use mklm_update::{Freshness, KeyId, SignatureSlot, TrustState, UpdateRefusal, Version};
use update_common::*;

fn run(
    github: &mut FakeGitHub,
    installed: &str,
    scratch: &Scratch,
    machine: &TrustState,
    now: u64,
) -> Result<CheckOutcome, CheckError> {
    let env = env(installed, scratch.0.clone());
    check(
        github,
        &env,
        &UpdateCache::new(scratch.0.clone()),
        machine,
        None,
        now,
        &AtomicBool::new(false),
    )
}

#[test]
fn a_new_release_is_offered_downloaded_and_recorded() {
    let scratch = Scratch::new("check-new");
    let cache = UpdateCache::new(scratch.0.clone());
    let mut github = FakeGitHub::new(new_release());
    let outcome = run(&mut github, "0.2.0", &scratch, &TrustState::default(), NOW).unwrap();
    let CheckOutcome::Available(offer) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(offer.verified.version, Version::new(0, 2, 1));
    assert_eq!(offer.slot, SignatureSlot::Main);
    assert_eq!(offer.verified.freshness, Freshness::Fresh);
    assert!(offer.downloaded.is_none());
    // The main signature came from the tag of the manifest's redirect (design m5b A.6).
    assert!(
        github
            .requests
            .iter()
            .any(|path| path.ends_with("/releases/download/v0.2.1/latest.json.minisig")),
        "{:?}",
        github.requests
    );
    // No alternate signature was looked for.
    assert!(
        !github
            .requests
            .iter()
            .any(|path| path.contains("alt.minisig"))
    );
    let state = cache.load_state();
    assert_eq!(state.last_success, Some(NOW));
    assert_eq!(
        state.trust.max_issued_at(KeyId::parse(PRIMARY_ID).unwrap()),
        Some(T1)
    );
    assert_eq!(
        cache.load_manifest().map(|(manifest, _)| manifest),
        Some(new_release().manifest.into_bytes())
    );
    // The download: the installer of this build's architecture, verified and renamed.
    let env = env("0.2.0", scratch.0.clone());
    let mut progress = Vec::new();
    let path = download(
        &mut github,
        &env,
        &cache,
        &offer,
        &mut |received, total| progress.push((received, total)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        installer_bytes(&offer.verified.asset.name).unwrap()
    );
    assert_eq!(
        progress.last().copied(),
        Some((offer.verified.asset.size, offer.verified.asset.size))
    );
    // Checked again: the offer knows the download, and the cached manifest verifies again.
    let outcome = run(
        &mut github,
        "0.2.0",
        &scratch,
        &TrustState::default(),
        NOW + 60,
    )
    .unwrap();
    assert!(
        matches!(outcome, CheckOutcome::Available(offer) if offer.downloaded == Some(path.clone()))
    );
    let again = reverify_cached(&env, &cache, &TrustState::default(), None, NOW + 120).unwrap();
    assert!(matches!(again, CheckOutcome::Available(_)));
}

#[test]
fn up_to_date_manual_and_expired() {
    let scratch = Scratch::new("check-offers");
    let mut github = FakeGitHub::new(new_release());
    assert!(matches!(
        run(&mut github, "0.2.1", &scratch, &TrustState::default(), NOW),
        Ok(CheckOutcome::UpToDate(_))
    ));
    let mut github = FakeGitHub::new(manual_release());
    assert!(matches!(
        run(&mut github, "0.1.0", &scratch, &TrustState::default(), NOW),
        Ok(CheckOutcome::ManualRequired(_))
    ));
    // 181 days after it was issued: still offered, but expired (design m5b C.6).
    let scratch = Scratch::new("check-expired");
    let mut github = FakeGitHub::new(new_release());
    let outcome = run(
        &mut github,
        "0.2.0",
        &scratch,
        &TrustState::default(),
        T1 + 181 * 86_400,
    )
    .unwrap();
    assert_eq!(outcome.verified().freshness, Freshness::Expired);
}

/// A key transition: the main signature's key is unknown, the alternate one verifies (design m5b
/// C.3, B.7).
#[test]
fn the_alternate_signature_of_a_key_transition() {
    let scratch = Scratch::new("check-alt");
    let mut github = FakeGitHub::new(transition_release());
    let outcome = run(&mut github, "0.2.0", &scratch, &TrustState::default(), NOW).unwrap();
    let CheckOutcome::Available(offer) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(offer.slot, SignatureSlot::Alt);
    assert_eq!(
        UpdateCache::new(scratch.0.clone())
            .load_state()
            .cached_signature,
        Some(SignatureSlot::Alt)
    );
    // Without the alternate one: the main signature's refusal.
    let mut release = transition_release();
    release.alt = None;
    let scratch = Scratch::new("check-alt-missing");
    let mut github = FakeGitHub::new(release);
    assert!(matches!(
        run(&mut github, "0.2.0", &scratch, &TrustState::default(), NOW),
        Err(CheckError::Refused(UpdateRefusal::UnknownKey { .. }))
    ));
}

/// An older manifest after a newer one was recorded is ignored and noted (design m5b C.4, E.3).
#[test]
fn an_older_manifest_is_a_rollback() {
    let scratch = Scratch::new("check-rollback");
    let mut github = FakeGitHub::new(new_release());
    run(&mut github, "0.2.0", &scratch, &TrustState::default(), NOW).unwrap();
    let mut github = FakeGitHub::new(old_release());
    let error = run(
        &mut github,
        "0.1.0",
        &scratch,
        &TrustState::default(),
        NOW + 60,
    )
    .unwrap_err();
    assert_eq!(
        error,
        CheckError::Refused(UpdateRefusal::Rollback {
            issued_at: T0,
            seen: T1
        })
    );
    let state = UpdateCache::new(scratch.0.clone()).load_state();
    let rollback = state.last_rollback.unwrap();
    assert_eq!((rollback.issued_at, rollback.seen), (T0, T1));
    assert_eq!(
        state.last_failure.map(|failure| failure.class),
        Some(ErrorClass::Structural)
    );
    // The machine's record alone refuses it too (a user who never checked, design m5b C.4).
    let fresh = Scratch::new("check-rollback-machine");
    let mut machine = TrustState::default();
    machine.max_issued_at.insert(PRIMARY_ID.into(), T1);
    let mut github = FakeGitHub::new(old_release());
    assert!(matches!(
        run(&mut github, "0.1.0", &fresh, &machine, NOW),
        Err(CheckError::Refused(UpdateRefusal::Rollback { .. }))
    ));
}
