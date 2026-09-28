//! The update check (design m5b A.5, C.3, C.4, E.1): fetch, verify, record, cache. Never
//! downloads the installer.
//!
//! One check:
//! 1. The manifest and its main signature (`fetch_manifest`: the tag of the first redirect).
//! 2. `verify_manifest` with `Purpose::Check` against the machine record merged with the user's
//!    (`TrustState::merged`); when the main signature fails with `UnknownKey` or `RevokedKey`
//!    (`tries_alternate`), the alternate signature of the same tag is fetched and tried. Without
//!    one, or when it fails too, the main signature's refusal is the answer (C.3).
//! 3. Success: the user record becomes `merged.recorded(verified, now)`; the manifest and the
//!    signature that verified are cached as received; `last_check` and `last_success` move, the
//!    run of failures ends.
//! 4. Failure: `classify` decides whether it counts (a cancellation, `NotConfigured` do not:
//!    `last_check` stays, FIX-VERIFICATION-12); a counted failure moves `last_check` and is kept
//!    in `last_failure` with the time its run began; an ignored older manifest also leaves a
//!    `last_rollback` (E.3).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mklm_update::fetch::{self, FetchError, FetchedManifest, Limits, Transport, TransportError};
use mklm_update::{
    OfferKind, Purpose, SignatureSlot, TrustState, UpdateRefusal, VerifiedManifest, VerifyInput,
    Version,
};

use crate::update::cache::{CheckFailure, ClientState, RollbackNote, UpdateCache};
use crate::update::classify::classify;
use crate::update::env::{Availability, UpdateEnv};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub verified: VerifiedManifest,
    pub manifest: Vec<u8>,
    /// The signature that verified (main or alternate).
    pub signature: Vec<u8>,
    pub slot: SignatureSlot,
    pub skipped: bool,
    /// The cached installer whose size and SHA-256 match, if any.
    pub downloaded: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate(VerifiedManifest),
    Available(Offer),
    ManualRequired(VerifiedManifest),
}

impl CheckOutcome {
    /// The verified manifest, whatever the offer.
    pub fn verified(&self) -> &VerifiedManifest {
        match self {
            CheckOutcome::UpToDate(verified) | CheckOutcome::ManualRequired(verified) => verified,
            CheckOutcome::Available(offer) => &offer.verified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    Unavailable(Availability),
    Fetch(FetchError),
    Refused(UpdateRefusal),
    Cache(String),
}

impl std::fmt::Display for CheckError {
    /// English, for logs, the CLI and the technical details.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Unavailable(Availability::Available) => write!(f, "available"),
            CheckError::Unavailable(Availability::NotConfigured) => {
                write!(f, "this build has no update keys")
            }
            CheckError::Unavailable(Availability::NotInstalledCopy {
                exe_dir,
                install_dir,
            }) => write!(
                f,
                "MKLM runs from {}, not from its installation folder {}",
                exe_dir.display(),
                install_dir.display()
            ),
            CheckError::Unavailable(Availability::Unknown { detail }) => {
                write!(f, "whether updates can be used is unknown: {detail}")
            }
            CheckError::Fetch(error) => {
                write!(f, "fetching the update information failed: {error}")
            }
            CheckError::Refused(refusal) => {
                write!(f, "the update information was refused: {refusal}")
            }
            CheckError::Cache(detail) => write!(f, "the update cache: {detail}"),
        }
    }
}

/// Fetch, verify (`Purpose::Check`, machine ∪ user state; the alternate signature when
/// `tries_alternate`), record the user state (including `last_failure` and `last_rollback`),
/// cache the manifest. Never downloads the installer.
pub fn check(
    transport: &mut dyn Transport,
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
    cancel: &AtomicBool,
) -> Result<CheckOutcome, CheckError> {
    check_with(
        &mut Real { transport },
        env,
        cache,
        machine,
        skipped,
        now_unix,
        cancel,
    )
}

/// The cached manifest verified again (before "update now", after a restart).
pub fn reverify_cached(
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
) -> Result<CheckOutcome, CheckError> {
    reverify_with(&Verifier, env, cache, machine, skipped, now_unix)
}

/// What a check needs of `mklm_update` besides its own rules: the fetches, the verification and
/// the record's rules. The real one is [`Real`]; tests replace it, so that the rules of this module
/// are tested without signed files or a network.
pub(crate) trait Checker {
    fn fetch(
        &mut self,
        env: &UpdateEnv,
        cancel: &AtomicBool,
    ) -> Result<FetchedManifest, FetchError>;
    fn fetch_alt(
        &mut self,
        env: &UpdateEnv,
        tag: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Option<Vec<u8>>, FetchError>;
    fn verifier(&self) -> &dyn Verify;
}

/// The verification and the record's rules (`mklm_update`).
pub(crate) trait Verify {
    fn verify(
        &self,
        env: &UpdateEnv,
        manifest: &[u8],
        signature: &[u8],
        state: &TrustState,
        tag: Option<&str>,
        now_unix: u64,
    ) -> Result<VerifiedManifest, UpdateRefusal>;
    fn tries_alternate(&self, error: &UpdateRefusal) -> bool;
    fn merged(&self, machine: &TrustState, user: &TrustState) -> TrustState;
    fn recorded(
        &self,
        state: &TrustState,
        verified: &VerifiedManifest,
        now_unix: u64,
    ) -> TrustState;
}

/// `mklm_update` itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Verifier;

impl Verify for Verifier {
    fn verify(
        &self,
        env: &UpdateEnv,
        manifest: &[u8],
        signature: &[u8],
        state: &TrustState,
        tag: Option<&str>,
        now_unix: u64,
    ) -> Result<VerifiedManifest, UpdateRefusal> {
        let anchors = env.anchors.as_ref().ok_or(UpdateRefusal::NotConfigured)?;
        mklm_update::verify_manifest(&VerifyInput {
            manifest,
            signature,
            anchors,
            state,
            installed: &env.installed,
            arch: env.arch,
            now_unix,
            tag,
            purpose: Purpose::Check,
        })
    }

    fn tries_alternate(&self, error: &UpdateRefusal) -> bool {
        mklm_update::tries_alternate(error)
    }

    fn merged(&self, machine: &TrustState, user: &TrustState) -> TrustState {
        machine.merged(user)
    }

    fn recorded(
        &self,
        state: &TrustState,
        verified: &VerifiedManifest,
        now_unix: u64,
    ) -> TrustState {
        state.recorded(verified, now_unix)
    }
}

/// The fetches over a real [`Transport`] with the limits of design m5b A.8.
struct Real<'t> {
    transport: &'t mut dyn Transport,
}

impl Checker for Real<'_> {
    fn fetch(
        &mut self,
        env: &UpdateEnv,
        cancel: &AtomicBool,
    ) -> Result<FetchedManifest, FetchError> {
        fetch::fetch_manifest(self.transport, &env.endpoints, &Limits::manifest(), cancel)
    }

    fn fetch_alt(
        &mut self,
        env: &UpdateEnv,
        tag: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Option<Vec<u8>>, FetchError> {
        fetch::fetch_alt_signature(
            self.transport,
            &env.endpoints,
            tag,
            &Limits::manifest(),
            cancel,
        )
    }

    fn verifier(&self) -> &dyn Verify {
        &Verifier
    }
}

/// What verified, with the bytes that did.
struct Verified {
    verified: VerifiedManifest,
    manifest: Vec<u8>,
    signature: Vec<u8>,
    slot: SignatureSlot,
}

/// [`check`] over any [`Checker`].
pub(crate) fn check_with(
    checker: &mut dyn Checker,
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
    cancel: &AtomicBool,
) -> Result<CheckOutcome, CheckError> {
    let mut client = cache.load_state();
    if let Some(error) = unavailable(env) {
        record_failure(&mut client, &error, now_unix);
        let _ = cache.save_state(&client);
        return Err(error);
    }
    let merged = checker.verifier().merged(machine, &client.trust);
    match fetch_and_verify(checker, env, &merged, now_unix, cancel) {
        Ok(found) => {
            client.trust = checker
                .verifier()
                .recorded(&merged, &found.verified, now_unix);
            client.last_check = Some(now_unix);
            client.last_success = Some(now_unix);
            client.last_failure = None;
            client.cached_signature = Some(found.slot);
            let cached = cache
                .store_manifest(&found.manifest, &found.signature)
                .and_then(|()| cache.save_state(&client));
            if let Err(error) = cached {
                return Err(CheckError::Cache(error.to_string()));
            }
            Ok(outcome(found, skipped, cache))
        }
        Err(error) => {
            record_failure(&mut client, &error, now_unix);
            // Not saving the record does not change what the check found.
            let _ = cache.save_state(&client);
            Err(error)
        }
    }
}

/// [`reverify_cached`] over any [`Verify`].
pub(crate) fn reverify_with(
    verifier: &dyn Verify,
    env: &UpdateEnv,
    cache: &UpdateCache,
    machine: &TrustState,
    skipped: Option<&Version>,
    now_unix: u64,
) -> Result<CheckOutcome, CheckError> {
    if let Some(error) = unavailable(env) {
        return Err(error);
    }
    let client = cache.load_state();
    let (manifest, signature) = cache
        .load_manifest()
        .ok_or_else(|| CheckError::Cache("no verified update information is cached".into()))?;
    let merged = verifier.merged(machine, &client.trust);
    let verified = verifier
        .verify(env, &manifest, &signature, &merged, None, now_unix)
        .map_err(CheckError::Refused)?;
    Ok(outcome(
        Verified {
            verified,
            manifest,
            signature,
            slot: client.cached_signature.unwrap_or(SignatureSlot::Main),
        },
        skipped,
        cache,
    ))
}

/// A check needs the keys and something to compare with: `NotConfigured` and `Unknown` stop it;
/// a copy outside the installation folder may check (design m5b E.8).
fn unavailable(env: &UpdateEnv) -> Option<CheckError> {
    match &env.availability {
        Availability::Available | Availability::NotInstalledCopy { .. } => None,
        other => Some(CheckError::Unavailable(other.clone())),
    }
}

/// The main signature, then — only after `UnknownKey` or `RevokedKey` — the alternate one of the
/// same tag (design m5b C.3).
fn fetch_and_verify(
    checker: &mut dyn Checker,
    env: &UpdateEnv,
    state: &TrustState,
    now_unix: u64,
    cancel: &AtomicBool,
) -> Result<Verified, CheckError> {
    let fetched = checker.fetch(env, cancel).map_err(CheckError::Fetch)?;
    let tag = fetched.tag.as_deref();
    let main = checker.verifier().verify(
        env,
        &fetched.manifest,
        &fetched.signature,
        state,
        tag,
        now_unix,
    );
    let main_error = match main {
        Ok(verified) => {
            return Ok(Verified {
                verified,
                manifest: fetched.manifest,
                signature: fetched.signature,
                slot: SignatureSlot::Main,
            });
        }
        Err(error) => error,
    };
    if !checker.verifier().tries_alternate(&main_error) {
        return Err(CheckError::Refused(main_error));
    }
    let alternate = match checker.fetch_alt(env, tag, cancel) {
        Ok(Some(alternate)) => alternate,
        // No alternate signature, or it could not be fetched: the main one's refusal.
        Ok(None) => return Err(CheckError::Refused(main_error)),
        Err(error @ (FetchError::Cancelled | FetchError::Transport(TransportError::Cancelled))) => {
            return Err(CheckError::Fetch(error));
        }
        Err(_) => return Err(CheckError::Refused(main_error)),
    };
    match checker
        .verifier()
        .verify(env, &fetched.manifest, &alternate, state, tag, now_unix)
    {
        Ok(verified) => Ok(Verified {
            verified,
            manifest: fetched.manifest,
            signature: alternate,
            slot: SignatureSlot::Alt,
        }),
        Err(_) => Err(CheckError::Refused(main_error)),
    }
}

/// The offer a verified manifest makes.
fn outcome(found: Verified, skipped: Option<&Version>, cache: &UpdateCache) -> CheckOutcome {
    match found.verified.offer {
        OfferKind::UpToDate => CheckOutcome::UpToDate(found.verified),
        OfferKind::ManualRequired => CheckOutcome::ManualRequired(found.verified),
        OfferKind::Newer => CheckOutcome::Available(Offer {
            skipped: skipped == Some(&found.verified.version),
            downloaded: cache.matching_installer(&found.verified.asset),
            verified: found.verified,
            manifest: found.manifest,
            signature: found.signature,
            slot: found.slot,
        }),
    }
}

/// A failed check in the user's record (design m5b E.3, E.6): an ignored older manifest, and the
/// failure itself when it counts.
fn record_failure(client: &mut ClientState, error: &CheckError, now_unix: u64) {
    if let CheckError::Refused(UpdateRefusal::Rollback { issued_at, seen }) = error {
        client.last_rollback = Some(RollbackNote {
            issued_at: *issued_at,
            seen: *seen,
            at: now_unix,
        });
    }
    let first_at = client
        .last_failure
        .as_ref()
        .map_or(now_unix, |failure| failure.first_at.min(now_unix));
    let days = now_unix.saturating_sub(first_at) / 86_400;
    if let Some((class, message_id)) = classify(error, days) {
        client.last_check = Some(now_unix);
        client.last_failure = Some(CheckFailure {
            class,
            message_id: message_id.to_string(),
            first_at,
            at: now_unix,
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::fs;

    use mklm_update::url::Endpoints;
    use mklm_update::{
        Arch, Freshness, KeyFingerprint, KeyId, KeyRole, SelectedAsset, Sha256Digest,
    };

    use super::*;
    use crate::update::cache::ErrorClass;
    use crate::update::cache::tests::Scratch;

    pub(crate) const NOW: u64 = 1_792_026_000;
    pub(crate) const ISSUED: u64 = 1_792_022_400;
    const PRIMARY: KeyId = KeyId([1, 2, 3, 4, 5, 6, 7, 8]);
    const BACKUP: KeyId = KeyId([9, 9, 9, 9, 9, 9, 9, 9]);

    /// An installer's bytes (a pattern) and its asset.
    pub(crate) fn installer(version: &Version) -> (Vec<u8>, SelectedAsset) {
        let bytes: Vec<u8> = (0..150_000u32).map(|i| (i % 251) as u8).collect();
        let asset = SelectedAsset {
            arch: Arch::of_this_build(),
            name: mklm_update::installer_name(version, Arch::of_this_build()),
            size: bytes.len() as u64,
            sha256: Sha256Digest::of(&bytes),
        };
        (bytes, asset)
    }

    /// A verified manifest of `version` as `verify_manifest` would return it.
    pub(crate) fn verified(version: &str, offer: OfferKind) -> VerifiedManifest {
        let version = Version::parse(version).unwrap();
        VerifiedManifest {
            asset: installer(&version).1,
            version,
            issued_at: ISSUED,
            expires: ISSUED + 180 * 86_400,
            signer: PRIMARY,
            signer_role: KeyRole::Primary,
            signer_fingerprint: KeyFingerprint([7; 32]),
            signer_is_dev: false,
            key_ids: vec![PRIMARY],
            revoked: BTreeMap::new(),
            min_from_version: None,
            freshness: Freshness::Fresh,
            offer,
        }
    }

    pub(crate) fn env(availability: Availability) -> UpdateEnv {
        UpdateEnv {
            installed: Version::new(0, 2, 0),
            arch: Arch::of_this_build(),
            native: None,
            availability,
            anchors: None,
            endpoints: Endpoints::production(),
            install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
            cache_dir: PathBuf::new(),
        }
    }

    /// The manifest bytes a fake answer carries: which verification to give.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Answer {
        Verified(Box<VerifiedManifest>),
        Refused(UpdateRefusal),
    }

    /// Plays the release and `mklm_update`: signatures name the answer they get.
    #[derive(Debug, Default)]
    pub(crate) struct FakeChecker {
        pub fetches: VecDeque<Result<FetchedManifest, FetchError>>,
        pub alt: Option<Result<Option<Vec<u8>>, FetchError>>,
        /// Signature bytes → the verification's answer.
        pub answers: BTreeMap<Vec<u8>, Answer>,
        pub alt_fetched: u32,
    }

    impl FakeChecker {
        pub(crate) fn release(signature: &str, answer: Answer) -> FakeChecker {
            let mut checker = FakeChecker::default();
            checker.fetches.push_back(Ok(FetchedManifest {
                manifest: b"{\"version\":\"0.2.1\"}".to_vec(),
                signature: signature.as_bytes().to_vec(),
                tag: Some("v0.2.1".into()),
            }));
            checker
                .answers
                .insert(signature.as_bytes().to_vec(), answer);
            checker
        }
    }

    impl Verify for FakeChecker {
        fn verify(
            &self,
            _env: &UpdateEnv,
            _manifest: &[u8],
            signature: &[u8],
            _state: &TrustState,
            _tag: Option<&str>,
            _now: u64,
        ) -> Result<VerifiedManifest, UpdateRefusal> {
            match self.answers.get(signature) {
                Some(Answer::Verified(verified)) => Ok((**verified).clone()),
                Some(Answer::Refused(refusal)) => Err(refusal.clone()),
                None => Err(UpdateRefusal::SignatureMalformed),
            }
        }

        fn tries_alternate(&self, error: &UpdateRefusal) -> bool {
            matches!(
                error,
                UpdateRefusal::UnknownKey { .. } | UpdateRefusal::RevokedKey { .. }
            )
        }

        /// Per key the higher value (the rule of `TrustState::merged`, simplified).
        fn merged(&self, machine: &TrustState, user: &TrustState) -> TrustState {
            let mut merged = machine.clone();
            for (key, value) in &user.max_issued_at {
                let entry = merged.max_issued_at.entry(key.clone()).or_insert(0);
                *entry = (*entry).max(*value);
            }
            merged
        }

        fn recorded(
            &self,
            state: &TrustState,
            verified: &VerifiedManifest,
            now: u64,
        ) -> TrustState {
            let mut state = state.clone();
            let entry = state
                .max_issued_at
                .entry(verified.signer.to_text())
                .or_insert(0);
            *entry = (*entry).max(verified.issued_at.min(now));
            state
        }
    }

    impl Checker for FakeChecker {
        fn fetch(
            &mut self,
            _env: &UpdateEnv,
            _cancel: &AtomicBool,
        ) -> Result<FetchedManifest, FetchError> {
            self.fetches
                .pop_front()
                .unwrap_or(Err(FetchError::NotFound))
        }

        fn fetch_alt(
            &mut self,
            _env: &UpdateEnv,
            _tag: Option<&str>,
            _cancel: &AtomicBool,
        ) -> Result<Option<Vec<u8>>, FetchError> {
            self.alt_fetched += 1;
            self.alt.clone().unwrap_or(Ok(None))
        }

        fn verifier(&self) -> &dyn Verify {
            self
        }
    }

    fn run(
        checker: &mut FakeChecker,
        cache: &UpdateCache,
        skipped: Option<&Version>,
        now: u64,
    ) -> Result<CheckOutcome, CheckError> {
        check_with(
            checker,
            &env(Availability::Available),
            cache,
            &TrustState::default(),
            skipped,
            now,
            &AtomicBool::new(false),
        )
    }

    fn newer() -> Answer {
        Answer::Verified(Box::new(verified("0.2.1", OfferKind::Newer)))
    }

    #[test]
    fn a_new_version_is_offered_recorded_and_cached() {
        let scratch = Scratch::new("check-new");
        let cache = scratch.cache();
        let mut checker = FakeChecker::release("main", newer());
        let outcome = run(&mut checker, &cache, None, NOW).unwrap();
        let CheckOutcome::Available(offer) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(offer.slot, SignatureSlot::Main);
        assert_eq!(offer.signature, b"main");
        assert!(!offer.skipped && offer.downloaded.is_none());
        assert_eq!(checker.alt_fetched, 0);
        let state = cache.load_state();
        assert_eq!(state.last_check, Some(NOW));
        assert_eq!(state.last_success, Some(NOW));
        assert_eq!(state.last_failure, None);
        assert_eq!(state.cached_signature, Some(SignatureSlot::Main));
        // The record keeps min(issued_at, now) for the signer.
        assert_eq!(
            state.trust.max_issued_at.get(&PRIMARY.to_text()),
            Some(&ISSUED)
        );
        assert_eq!(
            cache.load_manifest(),
            Some((b"{\"version\":\"0.2.1\"}".to_vec(), b"main".to_vec()))
        );
    }

    #[test]
    fn skipped_and_already_downloaded_versions() {
        let scratch = Scratch::new("check-skip");
        let cache = scratch.cache();
        let version = Version::new(0, 2, 1);
        let (bytes, asset) = installer(&version);
        fs::create_dir_all(&scratch.0).unwrap();
        fs::write(cache.installer_path(&asset.name), &bytes).unwrap();
        let mut checker = FakeChecker::release("main", newer());
        let Ok(CheckOutcome::Available(offer)) = run(&mut checker, &cache, Some(&version), NOW)
        else {
            panic!("no offer");
        };
        assert!(offer.skipped);
        assert_eq!(offer.downloaded, Some(cache.installer_path(&asset.name)));
        // A file that does not match is not the download.
        fs::write(cache.installer_path(&asset.name), b"short").unwrap();
        let mut checker = FakeChecker::release("main", newer());
        let Ok(CheckOutcome::Available(offer)) = run(&mut checker, &cache, None, NOW) else {
            panic!("no offer");
        };
        assert!(!offer.skipped);
        assert_eq!(offer.downloaded, None);
    }

    #[test]
    fn up_to_date_and_manual_updates() {
        let scratch = Scratch::new("check-offers");
        let cache = scratch.cache();
        let mut checker = FakeChecker::release(
            "main",
            Answer::Verified(Box::new(verified("0.2.0", OfferKind::UpToDate))),
        );
        assert!(matches!(
            run(&mut checker, &cache, None, NOW),
            Ok(CheckOutcome::UpToDate(v)) if v.version == Version::new(0, 2, 0)
        ));
        let mut checker = FakeChecker::release(
            "main",
            Answer::Verified(Box::new(verified("0.2.1", OfferKind::ManualRequired))),
        );
        let outcome = run(&mut checker, &cache, None, NOW).unwrap();
        assert!(matches!(outcome, CheckOutcome::ManualRequired(_)));
        assert_eq!(outcome.verified().version, Version::new(0, 2, 1));
    }

    /// The alternate signature only after `UnknownKey` / `RevokedKey`, and the main refusal when
    /// it does not help (design m5b C.3).
    #[test]
    fn the_alternate_signature() {
        let scratch = Scratch::new("check-alt");
        let cache = scratch.cache();
        let unknown = Answer::Refused(UpdateRefusal::UnknownKey {
            key_id: "5EE134AEFB6D6E15".into(),
        });
        // Main fails with an unknown key; the alternate verifies.
        let mut checker = FakeChecker::release("main", unknown.clone());
        checker.alt = Some(Ok(Some(b"alt".to_vec())));
        let mut backup = verified("0.2.1", OfferKind::Newer);
        backup.signer = BACKUP;
        backup.signer_role = KeyRole::Backup;
        checker
            .answers
            .insert(b"alt".to_vec(), Answer::Verified(Box::new(backup)));
        let Ok(CheckOutcome::Available(offer)) = run(&mut checker, &cache, None, NOW) else {
            panic!("no offer");
        };
        assert_eq!(
            (offer.slot, offer.signature.as_slice()),
            (SignatureSlot::Alt, &b"alt"[..])
        );
        assert_eq!(
            cache.load_state().cached_signature,
            Some(SignatureSlot::Alt)
        );
        assert_eq!(cache.load_manifest().unwrap().1, b"alt");
        // No alternate (404): the main refusal.
        let mut checker = FakeChecker::release("main", unknown.clone());
        assert_eq!(
            run(&mut checker, &cache, None, NOW),
            Err(CheckError::Refused(UpdateRefusal::UnknownKey {
                key_id: "5EE134AEFB6D6E15".into()
            }))
        );
        assert_eq!(checker.alt_fetched, 1);
        // The alternate fails too, or cannot be fetched: the main refusal.
        for alt in [
            Ok(Some(b"bad".to_vec())),
            Err(FetchError::HttpStatus { status: 500 }),
        ] {
            let mut checker = FakeChecker::release("main", unknown.clone());
            checker.alt = Some(alt);
            assert!(matches!(
                run(&mut checker, &cache, None, NOW),
                Err(CheckError::Refused(UpdateRefusal::UnknownKey { .. }))
            ));
        }
        // Cancelled while fetching it: cancelled.
        let mut checker = FakeChecker::release("main", unknown);
        checker.alt = Some(Err(FetchError::Cancelled));
        assert_eq!(
            run(&mut checker, &cache, None, NOW),
            Err(CheckError::Fetch(FetchError::Cancelled))
        );
        // Any other refusal never looks for another signature.
        let mut checker =
            FakeChecker::release("main", Answer::Refused(UpdateRefusal::BadSignature));
        checker.alt = Some(Ok(Some(b"alt".to_vec())));
        assert_eq!(
            run(&mut checker, &cache, None, NOW),
            Err(CheckError::Refused(UpdateRefusal::BadSignature))
        );
        assert_eq!(checker.alt_fetched, 0);
    }

    /// The run of failures, its first time and its class; what does not count (FIX-VERIFICATION-12).
    #[test]
    fn failures_are_recorded_with_the_start_of_their_run() {
        let scratch = Scratch::new("check-fail");
        let cache = scratch.cache();
        let day = 86_400;
        let mut checker = FakeChecker::default();
        checker
            .fetches
            .push_back(Err(FetchError::Transport(TransportError::Timeout)));
        checker.fetches.push_back(Err(FetchError::NotFound));
        checker.fetches.push_back(Err(FetchError::NotFound));
        checker.fetches.push_back(Err(FetchError::Cancelled));
        assert!(run(&mut checker, &cache, None, NOW).is_err());
        let state = cache.load_state();
        assert_eq!(state.last_check, Some(NOW));
        assert_eq!(
            state.last_failure,
            Some(CheckFailure {
                class: ErrorClass::Transient,
                message_id: "upd-net".into(),
                first_at: NOW,
                at: NOW,
            })
        );
        assert_eq!(state.last_success, None);
        // Not found, 3 days later: still transient, the run began at NOW.
        let _ = run(&mut checker, &cache, None, NOW + 3 * day);
        let failure = cache.load_state().last_failure.unwrap();
        assert_eq!(
            (failure.message_id.as_str(), failure.first_at),
            ("upd-not-found", NOW)
        );
        // 8 days after the run began: structural.
        let _ = run(&mut checker, &cache, None, NOW + 8 * day);
        let failure = cache.load_state().last_failure.unwrap();
        assert_eq!(
            (failure.class, failure.message_id.as_str()),
            (ErrorClass::Structural, "upd-not-found-long")
        );
        // A cancellation changes nothing.
        let before = cache.load_state();
        assert_eq!(
            run(&mut checker, &cache, None, NOW + 9 * day),
            Err(CheckError::Fetch(FetchError::Cancelled))
        );
        assert_eq!(cache.load_state(), before);
        // A success ends the run.
        checker.fetches.push_back(Ok(FetchedManifest {
            manifest: b"m".to_vec(),
            signature: b"main".to_vec(),
            tag: None,
        }));
        checker.answers.insert(b"main".to_vec(), newer());
        assert!(run(&mut checker, &cache, None, NOW + 10 * day).is_ok());
        let state = cache.load_state();
        assert_eq!(state.last_failure, None);
        assert_eq!(state.last_success, Some(NOW + 10 * day));
    }

    #[test]
    fn an_older_manifest_is_noted_and_counts_as_structural() {
        let scratch = Scratch::new("check-rollback");
        let cache = scratch.cache();
        let mut checker = FakeChecker::release(
            "main",
            Answer::Refused(UpdateRefusal::Rollback {
                issued_at: ISSUED - 86_400,
                seen: ISSUED,
            }),
        );
        assert!(matches!(
            run(&mut checker, &cache, None, NOW),
            Err(CheckError::Refused(UpdateRefusal::Rollback { .. }))
        ));
        let state = cache.load_state();
        assert_eq!(
            state.last_rollback,
            Some(RollbackNote {
                issued_at: ISSUED - 86_400,
                seen: ISSUED,
                at: NOW,
            })
        );
        assert_eq!(
            state.last_failure.map(|f| (f.class, f.message_id)),
            Some((ErrorClass::Structural, "upd-rollback".to_string()))
        );
    }

    #[test]
    fn unavailable_builds_do_not_fetch() {
        let scratch = Scratch::new("check-unavailable");
        let cache = scratch.cache();
        let mut checker = FakeChecker::release("main", newer());
        let result = check_with(
            &mut checker,
            &env(Availability::NotConfigured),
            &cache,
            &TrustState::default(),
            None,
            NOW,
            &AtomicBool::new(false),
        );
        assert_eq!(
            result,
            Err(CheckError::Unavailable(Availability::NotConfigured))
        );
        // Not a failure: nothing recorded.
        assert_eq!(cache.load_state(), ClientState::default());
        assert_eq!(checker.fetches.len(), 1);
        // A development copy checks.
        let copy = env(Availability::NotInstalledCopy {
            exe_dir: PathBuf::from(r"D:\dev"),
            install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
        });
        assert!(
            check_with(
                &mut checker,
                &copy,
                &cache,
                &TrustState::default(),
                None,
                NOW,
                &AtomicBool::new(false)
            )
            .is_ok()
        );
    }

    #[test]
    fn the_cached_manifest_is_verified_again() {
        let scratch = Scratch::new("check-reverify");
        let cache = scratch.cache();
        let checker = FakeChecker::release("main", newer());
        let env = env(Availability::Available);
        assert!(matches!(
            reverify_with(&checker, &env, &cache, &TrustState::default(), None, NOW),
            Err(CheckError::Cache(_))
        ));
        cache.store_manifest(b"m", b"main").unwrap();
        let skipped = Version::new(0, 2, 1);
        let Ok(CheckOutcome::Available(offer)) = reverify_with(
            &checker,
            &env,
            &cache,
            &TrustState::default(),
            Some(&skipped),
            NOW,
        ) else {
            panic!("no offer");
        };
        assert!(offer.skipped);
        assert_eq!(offer.manifest, b"m");
        // It records nothing.
        assert_eq!(cache.load_state(), ClientState::default());
    }

    #[test]
    fn the_english_diagnostics() {
        assert_eq!(
            CheckError::Unavailable(Availability::NotConfigured).to_string(),
            "this build has no update keys"
        );
        assert!(
            CheckError::Fetch(FetchError::NotFound)
                .to_string()
                .contains("not found")
        );
    }
}
