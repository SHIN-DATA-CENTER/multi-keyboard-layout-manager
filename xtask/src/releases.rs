//! The releases in the transition window (design m5b B.3 "窓の中の版"; FIX-VERIFICATION-2) and the
//! strand check on key IDs (B.3 step 10).
//!
//! A release stays a target until `TRANSITION_WINDOW_DAYS` after its **successor** was published:
//! its users learn new keys when they check after the successor appeared, not while it was the
//! latest. `prepare-release`, `publish` (through `prepare.json`) and `key-drill check` use the same
//! function.

use mklm_update::verify::check_key_rules;
use mklm_update::version::parse_release_version;
use mklm_update::{KeyId, TRANSITION_WINDOW_DAYS, TrustAnchors, UpdateRefusal, Version};

use crate::time::DAY;

/// One published release as `gh release list --exclude-drafts` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedRelease {
    pub tag: String,
    /// Unix seconds.
    pub published_at: u64,
    /// Also true for a stable release marked as a pre-release after publishing (B.5).
    pub is_prerelease: bool,
}

/// A release whose users must still accept the next manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTarget {
    pub tag: String,
    pub version: Version,
    pub published_at: u64,
    /// `None`: it is the latest release.
    pub successor_published_at: Option<u64>,
}

impl WindowTarget {
    /// The day it leaves the window, if it has a successor.
    pub fn window_ends(&self) -> Option<u64> {
        self.successor_published_at
            .map(|successor| successor + TRANSITION_WINDOW_DAYS * DAY)
    }
}

/// `vX.Y.Z` → `X.Y.Z` (release versions only: no pre-release tags).
pub fn stable_tag_version(tag: &str) -> Option<Version> {
    parse_release_version(tag.strip_prefix('v')?).ok()
}

/// The releases in the window at `now`: stable-shaped tags only (including those marked as
/// pre-releases after publishing; tags with `-` are left out), ordered by publication; the latest
/// one and every one whose successor was published less than `TRANSITION_WINDOW_DAYS` ago.
pub fn window_targets(releases: &[PublishedRelease], now: u64) -> Vec<WindowTarget> {
    let mut stable: Vec<(&PublishedRelease, Version)> = releases
        .iter()
        .filter_map(|release| stable_tag_version(&release.tag).map(|version| (release, version)))
        .collect();
    stable.sort_by(|(a, va), (b, vb)| {
        a.published_at
            .cmp(&b.published_at)
            .then_with(|| va.cmp_precedence(vb))
    });
    let mut targets = Vec::new();
    for (index, (release, version)) in stable.iter().enumerate() {
        let successor = stable.get(index + 1).map(|(next, _)| next.published_at);
        let in_window = match successor {
            None => true,
            Some(successor) => now < successor + TRANSITION_WINDOW_DAYS * DAY,
        };
        if in_window {
            targets.push(WindowTarget {
                tag: release.tag.clone(),
                version: version.clone(),
                published_at: release.published_at,
                successor_published_at: successor,
            });
        }
    }
    targets
}

/// One row of the strand table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrandRow {
    pub tag: String,
    pub main: Result<(), UpdateRefusal>,
    /// `None` without an alternate signature.
    pub alt: Option<Result<(), UpdateRefusal>>,
}

impl StrandRow {
    /// A build with these anchors accepts the manifest through at least one signature.
    pub fn accepted(&self) -> bool {
        self.main.is_ok() || matches!(self.alt, Some(Ok(())))
    }
}

/// `xtask::strand::accepts` of design m5b B.3 step 10: the key selection and revocation rules of
/// `verify_manifest`, on key IDs instead of signatures, for one release's trust anchors.
pub fn strand_row(
    tag: &str,
    anchors: &TrustAnchors,
    main: KeyId,
    alt: Option<KeyId>,
    revoked_keys: &[KeyId],
) -> StrandRow {
    StrandRow {
        tag: tag.to_string(),
        main: check_key_rules(anchors, main, revoked_keys),
        alt: alt.map(|alt| check_key_rules(anchors, alt, revoked_keys)),
    }
}

/// `○` / `× (reason)` for the printed table.
pub fn verdict(result: &Result<(), UpdateRefusal>) -> String {
    match result {
        Ok(()) => "ok".to_string(),
        Err(UpdateRefusal::UnknownKey { .. }) => "no (unknown key)".to_string(),
        Err(UpdateRefusal::RevokedKey { .. }) => "no (revoked key)".to_string(),
        Err(UpdateRefusal::IllegalRevocation { .. }) => {
            "no (revokes its own signing key)".to_string()
        }
        Err(other) => format!("no ({other})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_rfc3339_utc;

    fn release(tag: &str, date: &str) -> PublishedRelease {
        PublishedRelease {
            tag: tag.to_string(),
            published_at: parse_rfc3339_utc(&format!("{date}T00:00:00Z")).unwrap(),
            is_prerelease: false,
        }
    }

    fn at(date: &str) -> u64 {
        parse_rfc3339_utc(&format!("{date}T00:00:00Z")).unwrap()
    }

    fn tags(targets: &[WindowTarget]) -> Vec<&str> {
        targets.iter().map(|target| target.tag.as_str()).collect()
    }

    /// The example of design m5b B.3: v0.4.0 was the latest for a year; 60 days after v0.5.0 it
    /// is still in the window.
    #[test]
    fn the_example_of_design_b3() {
        let releases = [
            release("v0.4.0", "2027-01-10"),
            release("v0.5.0", "2028-01-10"),
        ];
        let targets = window_targets(&releases, at("2028-03-10"));
        assert_eq!(tags(&targets), ["v0.4.0", "v0.5.0"]);
        assert_eq!(targets[0].successor_published_at, Some(at("2028-01-10")));
        assert_eq!(targets[1].successor_published_at, None);
        assert_eq!(targets[0].window_ends(), Some(at("2028-01-10") + 400 * DAY));
        assert_eq!(targets[1].window_ends(), None);
    }

    #[test]
    fn the_window_is_400_days_after_the_successor() {
        let releases = [
            release("v0.4.0", "2027-01-10"),
            release("v0.5.0", "2028-01-10"),
        ];
        let successor = at("2028-01-10");
        assert_eq!(
            tags(&window_targets(&releases, successor + 399 * DAY)),
            ["v0.4.0", "v0.5.0"]
        );
        assert_eq!(
            tags(&window_targets(&releases, successor + 400 * DAY - 1)),
            ["v0.4.0", "v0.5.0"]
        );
        assert_eq!(
            tags(&window_targets(&releases, successor + 400 * DAY)),
            ["v0.5.0"]
        );
        assert_eq!(
            tags(&window_targets(&releases, successor + 401 * DAY)),
            ["v0.5.0"]
        );
    }

    #[test]
    fn pre_releases_and_marked_releases() {
        let mut marked = release("v0.3.0", "2027-06-01");
        marked.is_prerelease = true;
        let releases = [
            release("v0.2.0", "2026-10-15"),
            release("v0.3.0-rc.1", "2027-05-01"),
            marked,
            release("latest", "2027-06-02"),
            release("v0.3.1", "2027-07-01"),
        ];
        let targets = window_targets(&releases, at("2027-08-01"));
        // The release marked as a pre-release after publishing is a target; the real pre-release
        // and odd tags are not; v0.2.0's successor is v0.3.0.
        assert_eq!(tags(&targets), ["v0.2.0", "v0.3.0", "v0.3.1"]);
        assert_eq!(targets[0].successor_published_at, Some(at("2027-06-01")));
        assert!(window_targets(&[], at("2027-08-01")).is_empty());
    }
}
