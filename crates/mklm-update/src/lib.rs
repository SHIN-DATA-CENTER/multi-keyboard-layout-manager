//! Signed update manifests of MKLM (design docs/design/m5b-updater.md, sections A to C and H.1).
//!
//! OS-independent (except [`winhttp`]): the manifest (`latest.json`) and its minisign signatures,
//! the trust anchors embedded in the build, revocations and the anti-rollback record, the version
//! rules, the URL and fetch policy over a [`fetch::Transport`], the helper's receiving state
//! machine ([`stage`]), the records of one update run ([`run`]) and the pure driver of the update
//! runner ([`run_flow`]).
//!
//! The same code verifies in the GUI, the CLI, the elevated helper and `xtask`. Nothing here opens
//! a secret key: signing is done by the official minisign binary alone (design m5b B.1, B.5).
//!
//! Development overrides (a loopback URL policy, the development key) exist only under
//! `cfg(all(debug_assertions, mklm_update_dev))` (design m5b A.10).

#![forbid(unsafe_code)]

mod base64;
#[cfg(all(debug_assertions, mklm_update_dev))]
mod dev; // the development key and DEV_MARKER (design m5b A.10)
pub mod fetch;
pub mod gate;
pub mod keys;
pub mod manifest;
pub mod refusal;
pub mod run;
pub mod run_flow;
pub mod stage;
pub mod state;
pub mod url;
pub mod verify;
pub mod version;
#[cfg(all(windows, feature = "winhttp"))]
pub mod winhttp;

pub use keys::{AnchorEntry, AnchorsFile, KeyError, KeyFingerprint, KeyId, KeyRole, TrustAnchors};
pub use manifest::{Arch, Manifest, ManifestAsset, Sha256Digest, Sha256Stream};
pub use refusal::UpdateRefusal;
pub use semver::Version;
pub use state::{RevokedKey, StateError, TrustState};
pub use verify::{
    Freshness, OfferKind, Purpose, SelectedAsset, SignatureSlot, VerifiedManifest, VerifyInput,
    apply_trust_report, tries_alternate, verify_manifest,
};

pub const PRODUCT: &str = "MKLM";
pub const CHANNEL: &str = "stable";
pub const MANIFEST_SCHEMA: u32 = 1;
pub const MANIFEST_NAME: &str = "latest.json";
pub const SIGNATURE_NAME: &str = "latest.json.minisig";
/// The alternate signature, present during a key transition (SECURITY-4, OPS-UX-TEST-1).
pub const ALT_SIGNATURE_NAME: &str = "latest.json.alt.minisig";
pub const TRUSTED_COMMENT_PREFIX: &str = "mklm-latest-json v1";
/// Rehearsal manifests; accepted only from the development key in development builds.
pub const DEV_TRUSTED_COMMENT_PREFIX: &str = "mklm-dev-latest-json v1";
/// `xtask key-drill`; never accepted as a manifest (OPS-UX-TEST-13).
pub const KEY_DRILL_COMMENT_PREFIX: &str = "mklm-key-drill v1";
pub const REPO_URL: &str = "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager";
pub const MAX_MANIFEST_LEN: usize = 64 * 1024;
pub const MAX_SIGNATURE_LEN: usize = 4 * 1024;
pub const MAX_INSTALLER_LEN: u64 = 64 * 1024 * 1024;
pub const MAX_VALIDITY_SECS: u64 = 800 * 86_400;
/// xtask's default `--expires-days` (user decision J-3, 2026-09-29: 180 days; the canary warns 60
/// days before).
pub const DEFAULT_VALIDITY_DAYS: u64 = 180;
// The default validity fits the limit `verify_manifest` enforces.
const _: () = assert!(DEFAULT_VALIDITY_DAYS * 86_400 <= MAX_VALIDITY_SECS);

/// A release stays a strand-check target until this many days after its successor was published
/// (xtask `window_targets`, design m5b B.3; FIX-VERIFICATION-2).
pub const TRANSITION_WINDOW_DAYS: u64 = 400;
/// `key_ids` holds 1 or 2 IDs.
pub const MAX_KEY_IDS: usize = 2;
/// The GUI's start argument after an update (H2 and the RunOnce value pass it).
pub const GUI_AFTER_UPDATE_ARG: &str = "--after-update";
/// H2's file name in its run folder (RELIABILITY-12).
pub const RUNNER_EXE_NAME: &str = "mklm-update-runner.exe";

/// `MKLM-Setup-<version>-<arch>.exe`.
pub fn installer_name(version: &Version, arch: Arch) -> String {
    format!("MKLM-Setup-{version}-{}.exe", arch.as_str())
}

/// `<REPO_URL>/releases/tag/v<version>`.
pub fn release_page_url(version: &Version) -> String {
    format!("{REPO_URL}/releases/tag/v{version}")
}

/// `MKLM/<version> (Windows; <arch>; +<REPO_URL>)`.
pub fn user_agent(version: &str, arch: Arch) -> String {
    format!("MKLM/{version} (Windows; {}; +{REPO_URL})", arch.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_urls() {
        let version = Version::new(0, 2, 1);
        assert_eq!(
            installer_name(&version, Arch::X64),
            "MKLM-Setup-0.2.1-x64.exe"
        );
        assert_eq!(
            installer_name(&version, Arch::Arm64),
            "MKLM-Setup-0.2.1-arm64.exe"
        );
        assert_eq!(
            release_page_url(&version),
            "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/tag/v0.2.1"
        );
        assert_eq!(
            user_agent("0.2.0", Arch::X64),
            "MKLM/0.2.0 (Windows; x64; +https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager)"
        );
        assert_eq!(
            user_agent("0.2.0-dev.1", Arch::Arm64),
            "MKLM/0.2.0-dev.1 (Windows; arm64; +https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager)"
        );
        assert_eq!(RUNNER_EXE_NAME, "mklm-update-runner.exe");
        assert_eq!(GUI_AFTER_UPDATE_ARG, "--after-update");
    }

    #[test]
    fn limits_and_defaults() {
        // User decision J-3 (2026-09-29): manifests expire 180 days after they were issued.
        assert_eq!(DEFAULT_VALIDITY_DAYS, 180);
        assert_eq!(MAX_VALIDITY_SECS, 800 * 86_400);
        assert_eq!(TRANSITION_WINDOW_DAYS, 400);
        assert_eq!(MAX_MANIFEST_LEN, 65_536);
        assert_eq!(MAX_SIGNATURE_LEN, 4_096);
        assert_eq!(MAX_INSTALLER_LEN, 67_108_864);
        assert_eq!(MAX_KEY_IDS, 2);
        assert_eq!(SIGNATURE_NAME, format!("{MANIFEST_NAME}.minisig"));
        assert_eq!(ALT_SIGNATURE_NAME, format!("{MANIFEST_NAME}.alt.minisig"));
        for (a, b) in [
            (TRUSTED_COMMENT_PREFIX, DEV_TRUSTED_COMMENT_PREFIX),
            (TRUSTED_COMMENT_PREFIX, KEY_DRILL_COMMENT_PREFIX),
            (DEV_TRUSTED_COMMENT_PREFIX, KEY_DRILL_COMMENT_PREFIX),
        ] {
            assert!(!a.starts_with(b) && !b.starts_with(a), "{a} / {b}");
        }
    }
}
