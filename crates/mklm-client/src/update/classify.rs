//! Check failures as the user sees them (design m5b E.6): transient or structural, and the
//! message ID.
//!
//! The message IDs are those of design m5b E.6 ("upd-net", "upd-sig" …). The GUI words each
//! typed value itself (`apps/mklm/src/i18n/update.rs`); these IDs are what the user's record keeps
//! about the last failure (`CheckFailure::message_id`), so that the 30-day notice can say what
//! kind of failure it was (E.3), and what tests compare with the table.

use mklm_update::UpdateRefusal;
use mklm_update::fetch::{FetchError, TransportError};
use mklm_update::run::timing::NOT_FOUND_STRUCTURAL_DAYS;

use crate::update::cache::ErrorClass;
use crate::update::check::CheckError;
use crate::update::env::Availability;

/// Design m5b E.6: the class and message ID of a check failure. `not_found_days` = days since the
/// run of failures began (NotFound turns structural after `NOT_FOUND_STRUCTURAL_DAYS`). `None`
/// for what is not a failure (both `Cancelled`s, `Unavailable(NotConfigured)`,
/// `Unavailable(NotInstalledCopy)`): not recorded, `last_check` unchanged. `Rollback` is
/// `Structural` (FIX-VERIFICATION-12).
pub fn classify(error: &CheckError, not_found_days: u64) -> Option<(ErrorClass, &'static str)> {
    match error {
        CheckError::Unavailable(availability) => match availability {
            Availability::NotConfigured | Availability::NotInstalledCopy { .. } => None,
            Availability::Unknown { .. } => Some((ErrorClass::Transient, "upd-env-unknown")),
            // Never returned as an error; a bug if it is (E.6).
            Availability::Available => Some((ErrorClass::Transient, "upd-prepare-failed")),
        },
        CheckError::Fetch(error) => fetch_class(error, false, not_found_days),
        CheckError::Refused(refusal) => refusal_class(refusal),
        CheckError::Cache(_) => Some((ErrorClass::Transient, "upd-cache")),
    }
}

/// The class and message ID of a fetch failure; `download`: the installer's (the network row
/// says "download" instead of "check", `upd-net-dl`). `None` for a cancellation.
pub fn fetch_class(
    error: &FetchError,
    download: bool,
    not_found_days: u64,
) -> Option<(ErrorClass, &'static str)> {
    use ErrorClass::{Structural, Transient};
    let network = if download { "upd-net-dl" } else { "upd-net" };
    Some(match error {
        FetchError::Transport(transport) => match transport {
            TransportError::Timeout
            | TransportError::NameNotResolved
            | TransportError::CannotConnect
            | TransportError::Other { .. } => (Transient, network),
            TransportError::ProxyAuthRequired => (Structural, "upd-proxy-auth"),
            TransportError::Tls => (Transient, "upd-tls"),
            TransportError::Cancelled => return None,
        },
        FetchError::DeadlineExceeded => (Transient, network),
        FetchError::Cancelled => return None,
        FetchError::NotFound if not_found_days >= NOT_FOUND_STRUCTURAL_DAYS => {
            (Structural, "upd-not-found-long")
        }
        FetchError::NotFound => (Transient, "upd-not-found"),
        FetchError::RateLimited { .. } | FetchError::HttpStatus { .. } => {
            (Transient, "upd-gh-temp")
        }
        FetchError::TooManyRedirects
        | FetchError::RedirectNotAllowed { .. }
        | FetchError::MissingLocation
        | FetchError::UnexpectedEncoding => (Structural, "upd-gh-changed"),
        // The manifest or a signature over its limit: a format the build cannot take; the
        // installer's: a download that does not match (E.6).
        FetchError::TooLarge { .. } if download => (Transient, "upd-download-mismatch"),
        FetchError::TooLarge { .. } => (Structural, "upd-format"),
        FetchError::SizeMismatch { .. } | FetchError::HashMismatch => {
            (Transient, "upd-download-mismatch")
        }
        FetchError::Sink { .. } => (Transient, "upd-cache"),
    })
}

/// The class and message ID of a refusal as a check meets it. `None` for what is not a failure
/// (`NotConfigured`, `NotInstalledCopy`). What a check never returns (`NotNewer`,
/// `ManualUpdateRequired`, the helper's refusals) is `upd-prepare-failed`, and the caller logs it
/// as a bug (E.6).
pub fn refusal_class(refusal: &UpdateRefusal) -> Option<(ErrorClass, &'static str)> {
    use ErrorClass::{Structural, Transient};
    Some(match refusal {
        UpdateRefusal::NotConfigured | UpdateRefusal::NotInstalledCopy => return None,
        UpdateRefusal::SignatureTooLarge { .. }
        | UpdateRefusal::SignatureMalformed
        | UpdateRefusal::WrongTrustedComment
        | UpdateRefusal::UnknownKey { .. }
        | UpdateRefusal::BadSignature
        | UpdateRefusal::RevokedKey { .. }
        | UpdateRefusal::SignerNotListed { .. }
        | UpdateRefusal::IllegalRevocation { .. } => (Structural, "upd-sig"),
        UpdateRefusal::ManifestTooLarge { .. }
        | UpdateRefusal::ManifestMalformed { .. }
        | UpdateRefusal::UnsupportedSchema { .. }
        | UpdateRefusal::WrongProduct
        | UpdateRefusal::WrongChannel
        | UpdateRefusal::BadVersion { .. }
        | UpdateRefusal::BadTimestamps
        | UpdateRefusal::AssetMalformed { .. }
        | UpdateRefusal::NoAssetForArch { .. } => (Structural, "upd-format"),
        UpdateRefusal::TagMismatch { .. } => (Transient, "upd-race"),
        UpdateRefusal::Rollback { .. } => (Structural, "upd-rollback"),
        UpdateRefusal::NotNewer { .. }
        | UpdateRefusal::ManualUpdateRequired { .. }
        | UpdateRefusal::Busy
        | UpdateRefusal::UpdateInProgress
        | UpdateRefusal::OperationOpen { .. }
        | UpdateRefusal::RecoveryNeeded
        | UpdateRefusal::JournalUnreadable
        | UpdateRefusal::DiskFull { .. }
        | UpdateRefusal::InstallerSizeMismatch { .. }
        | UpdateRefusal::InstallerHashMismatch
        | UpdateRefusal::ChunkMalformed
        | UpdateRefusal::ChunkOutOfOrder { .. }
        | UpdateRefusal::CallerLeft
        | UpdateRefusal::HandOffFailed { .. }
        | UpdateRefusal::Storage { .. }
        | UpdateRefusal::Internal { .. } => (Transient, "upd-prepare-failed"),
    })
}

/// True when [`classify`] maps a refusal a check never returns (logged as a bug by the callers).
pub fn is_unexpected_in_check(refusal: &UpdateRefusal) -> bool {
    refusal_class(refusal).is_some_and(|(_, id)| id == "upd-prepare-failed")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mklm_update::Arch;

    use super::*;
    use ErrorClass::{Structural, Transient};

    fn refused(refusal: UpdateRefusal) -> CheckError {
        CheckError::Refused(refusal)
    }

    /// Every variant of `CheckError` against the table of design m5b E.6 (F.4).
    #[test]
    fn the_table_of_e6() {
        let detail = || "x".to_string();
        let rows: Vec<(CheckError, Option<(ErrorClass, &str)>)> = vec![
            // Not failures: not recorded.
            (CheckError::Fetch(FetchError::Cancelled), None),
            (
                CheckError::Fetch(FetchError::Transport(TransportError::Cancelled)),
                None,
            ),
            (CheckError::Unavailable(Availability::NotConfigured), None),
            (
                CheckError::Unavailable(Availability::NotInstalledCopy {
                    exe_dir: PathBuf::from(r"C:\dev\target\debug"),
                    install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
                }),
                None,
            ),
            (
                CheckError::Unavailable(Availability::Unknown { detail: detail() }),
                Some((Transient, "upd-env-unknown")),
            ),
            (
                CheckError::Unavailable(Availability::Available),
                Some((Transient, "upd-prepare-failed")),
            ),
            // The network.
            (
                CheckError::Fetch(FetchError::Transport(TransportError::Timeout)),
                Some((Transient, "upd-net")),
            ),
            (
                CheckError::Fetch(FetchError::Transport(TransportError::NameNotResolved)),
                Some((Transient, "upd-net")),
            ),
            (
                CheckError::Fetch(FetchError::Transport(TransportError::CannotConnect)),
                Some((Transient, "upd-net")),
            ),
            (
                CheckError::Fetch(FetchError::Transport(TransportError::Other {
                    code: 12_002,
                })),
                Some((Transient, "upd-net")),
            ),
            (
                CheckError::Fetch(FetchError::DeadlineExceeded),
                Some((Transient, "upd-net")),
            ),
            (
                CheckError::Fetch(FetchError::Transport(TransportError::ProxyAuthRequired)),
                Some((Structural, "upd-proxy-auth")),
            ),
            (
                CheckError::Fetch(FetchError::Transport(TransportError::Tls)),
                Some((Transient, "upd-tls")),
            ),
            (
                CheckError::Fetch(FetchError::NotFound),
                Some((Transient, "upd-not-found")),
            ),
            (
                CheckError::Fetch(FetchError::RateLimited { status: 429 }),
                Some((Transient, "upd-gh-temp")),
            ),
            (
                CheckError::Fetch(FetchError::HttpStatus { status: 503 }),
                Some((Transient, "upd-gh-temp")),
            ),
            (
                CheckError::Fetch(FetchError::TooManyRedirects),
                Some((Structural, "upd-gh-changed")),
            ),
            (
                CheckError::Fetch(FetchError::RedirectNotAllowed {
                    location: "https://example.com/".into(),
                }),
                Some((Structural, "upd-gh-changed")),
            ),
            (
                CheckError::Fetch(FetchError::MissingLocation),
                Some((Structural, "upd-gh-changed")),
            ),
            (
                CheckError::Fetch(FetchError::UnexpectedEncoding),
                Some((Structural, "upd-gh-changed")),
            ),
            (
                CheckError::Fetch(FetchError::TooLarge { limit: 65_536 }),
                Some((Structural, "upd-format")),
            ),
            (
                CheckError::Fetch(FetchError::SizeMismatch {
                    expected: 1,
                    received: 2,
                }),
                Some((Transient, "upd-download-mismatch")),
            ),
            (
                CheckError::Fetch(FetchError::HashMismatch),
                Some((Transient, "upd-download-mismatch")),
            ),
            (
                CheckError::Fetch(FetchError::Sink { detail: detail() }),
                Some((Transient, "upd-cache")),
            ),
            (CheckError::Cache(detail()), Some((Transient, "upd-cache"))),
            // Refusals.
            (
                refused(UpdateRefusal::SignatureTooLarge { len: 5000 }),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::SignatureMalformed),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::WrongTrustedComment),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::UnknownKey { key_id: detail() }),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::BadSignature),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::RevokedKey { key_id: detail() }),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::SignerNotListed { key_id: detail() }),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::IllegalRevocation { key_id: detail() }),
                Some((Structural, "upd-sig")),
            ),
            (
                refused(UpdateRefusal::ManifestTooLarge { len: 70_000 }),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::ManifestMalformed { detail: detail() }),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::UnsupportedSchema { schema: 2 }),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::WrongProduct),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::WrongChannel),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::BadVersion { text: detail() }),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::BadTimestamps),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::AssetMalformed { detail: detail() }),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::NoAssetForArch { arch: Arch::Arm64 }),
                Some((Structural, "upd-format")),
            ),
            (
                refused(UpdateRefusal::TagMismatch {
                    tag: "v0.2.2".into(),
                    version: "0.2.1".into(),
                }),
                Some((Transient, "upd-race")),
            ),
            (
                refused(UpdateRefusal::Rollback {
                    issued_at: 1,
                    seen: 2,
                }),
                Some((Structural, "upd-rollback")),
            ),
            (refused(UpdateRefusal::NotConfigured), None),
            (refused(UpdateRefusal::NotInstalledCopy), None),
        ];
        for (error, expected) in &rows {
            assert_eq!(classify(error, 0), *expected, "{error:?}");
        }
        // What a check never returns: a transient failure, and a bug to log.
        for refusal in [
            UpdateRefusal::NotNewer {
                offered: "0.2.0".into(),
                installed: "0.2.0".into(),
            },
            UpdateRefusal::ManualUpdateRequired {
                min_from: "0.2.0".into(),
                installed: "0.1.0".into(),
            },
            UpdateRefusal::Busy,
            UpdateRefusal::UpdateInProgress,
            UpdateRefusal::OperationOpen {
                waiting_for_reboot: true,
            },
            UpdateRefusal::RecoveryNeeded,
            UpdateRefusal::JournalUnreadable,
            UpdateRefusal::DiskFull {
                needed: 2,
                available: 1,
            },
            UpdateRefusal::InstallerSizeMismatch {
                expected: 2,
                received: 1,
            },
            UpdateRefusal::InstallerHashMismatch,
            UpdateRefusal::ChunkMalformed,
            UpdateRefusal::ChunkOutOfOrder {
                expected: 0,
                found: 1,
            },
            UpdateRefusal::CallerLeft,
            UpdateRefusal::HandOffFailed { detail: detail() },
            UpdateRefusal::Storage { detail: detail() },
            UpdateRefusal::Internal { detail: detail() },
        ] {
            assert!(is_unexpected_in_check(&refusal), "{refusal:?}");
            assert_eq!(
                classify(&refused(refusal), 0),
                Some((Transient, "upd-prepare-failed"))
            );
        }
        assert!(!is_unexpected_in_check(&UpdateRefusal::BadSignature));
    }

    /// "Not found" is structural once the failures have gone on for 7 days (E.6).
    #[test]
    fn not_found_turns_structural_after_seven_days() {
        let error = CheckError::Fetch(FetchError::NotFound);
        assert_eq!(classify(&error, 6), Some((Transient, "upd-not-found")));
        assert_eq!(
            classify(&error, 7),
            Some((Structural, "upd-not-found-long"))
        );
        // The download words the network failure as a download.
        assert_eq!(
            fetch_class(&FetchError::Transport(TransportError::Timeout), true, 0),
            Some((Transient, "upd-net-dl"))
        );
        assert_eq!(
            fetch_class(&FetchError::TooLarge { limit: 1 }, true, 0),
            Some((Transient, "upd-download-mismatch"))
        );
    }
}
