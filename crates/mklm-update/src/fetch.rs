//! The fetch policy over a [`Transport`] (design m5b A.5 to A.8): redirects one at a time under a
//! [`UrlPolicy`](crate::url::UrlPolicy), size limits, deadlines, cancellation, SHA-256.
//!
//! WP-0 writes the types and the limits of design m5b A.8; WP-U the fetches.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::Version;
use crate::url::{Endpoints, Url};
use crate::verify::SelectedAsset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub resolve: Duration,
    pub connect: Duration,
    pub send: Duration,
    pub receive: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeouts: Timeouts,
    /// Whole fetch, redirects included.
    pub total: Duration,
    pub max_redirects: u8,
}

impl Limits {
    /// 15 s / 15 s / 30 s / 30 s, total 60 s, 5 redirects (design m5b A.8). Also the alternate
    /// signature and fetch-smoke.
    pub fn manifest() -> Limits {
        Limits {
            timeouts: Timeouts {
                resolve: Duration::from_secs(15),
                connect: Duration::from_secs(15),
                send: Duration::from_secs(30),
                receive: Duration::from_secs(30),
            },
            total: Duration::from_secs(60),
            max_redirects: 5,
        }
    }

    /// 15 s / 15 s / 30 s / 60 s, total 30 min, 5 redirects.
    pub fn installer() -> Limits {
        Limits {
            timeouts: Timeouts {
                resolve: Duration::from_secs(15),
                connect: Duration::from_secs(15),
                send: Duration::from_secs(30),
                receive: Duration::from_secs(60),
            },
            total: Duration::from_secs(30 * 60),
            max_redirects: 5,
        }
    }
}

/// One GET without following redirects, cookies or credentials.
pub trait Transport {
    fn get(
        &mut self,
        url: &Url,
        accept: &str,
        timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError>;
}

pub trait Response {
    fn status(&self) -> u16;
    /// Case-insensitive name; `None` when absent.
    fn header(&self, name: &str) -> Option<String>;
    /// 0 at the end of the body.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("timed out")]
    Timeout,
    #[error("the host name could not be resolved")]
    NameNotResolved,
    #[error("could not connect")]
    CannotConnect,
    #[error("the secure connection failed")]
    Tls,
    #[error("the proxy requires authentication")]
    ProxyAuthRequired,
    #[error("cancelled")]
    Cancelled,
    #[error("network error {code}")]
    Other { code: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error(transparent)]
    Transport(TransportError),
    #[error("not found (404)")]
    NotFound,
    #[error("rate limited ({status})")]
    RateLimited { status: u16 },
    /// Includes 401 (server authentication is never answered, SECURITY-13).
    #[error("HTTP status {status}")]
    HttpStatus { status: u16 },
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("redirect to {location:?} is not allowed")]
    RedirectNotAllowed { location: String },
    #[error("a redirect without Location")]
    MissingLocation,
    #[error("an encoded (compressed) response")]
    UnexpectedEncoding,
    #[error("larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("received {received} bytes, expected {expected}")]
    SizeMismatch { expected: u64, received: u64 },
    #[error("SHA-256 mismatch")]
    HashMismatch,
    #[error("the fetch took too long")]
    DeadlineExceeded,
    #[error("cancelled")]
    Cancelled,
    #[error("writing the download failed: {detail}")]
    Sink { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedManifest {
    pub manifest: Vec<u8>,
    /// The main signature.
    pub signature: Vec<u8>,
    pub tag: Option<String>,
}

/// latest.json, then its main signature (from the same tag when known). Design m5b A.5–A.8.
pub fn fetch_manifest(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<FetchedManifest, FetchError> {
    Err(skeleton()) // Skeleton (M5b): WP-U
}

/// The alternate signature of the same tag; `Ok(None)` on 404 (design m5b A.5).
pub fn fetch_alt_signature(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    tag: Option<&str>,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Option<Vec<u8>>, FetchError> {
    Err(skeleton()) // Skeleton (M5b): WP-U
}

/// `releases/latest/download/<name>` of at most `max_len` bytes, and the tag of its first redirect
/// (xtask fetch-smoke, OPS-UX-TEST-8).
pub fn fetch_latest_file(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    name: &str,
    max_len: u64,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Option<String>), FetchError> {
    Err(skeleton()) // Skeleton (M5b): WP-U
}

/// Streams the installer into `sink`, checking the exact size and SHA-256; `progress(bytes)`.
#[allow(clippy::too_many_arguments)] // The signature of design m5b H.1.
pub fn download_asset(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    version: &Version,
    asset: &SelectedAsset,
    limits: &Limits,
    sink: &mut dyn std::io::Write,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), FetchError> {
    Err(skeleton()) // Skeleton (M5b): WP-U
}

/// What the WP-0 skeleton's unimplemented fetches return (design m5b G.2): `ERROR_NOT_SUPPORTED`.
fn skeleton() -> FetchError {
    FetchError::Transport(TransportError::Other { code: 50 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_of_design_a8() {
        let manifest = Limits::manifest();
        assert_eq!(manifest.timeouts.resolve, Duration::from_secs(15));
        assert_eq!(manifest.timeouts.connect, Duration::from_secs(15));
        assert_eq!(manifest.timeouts.send, Duration::from_secs(30));
        assert_eq!(manifest.timeouts.receive, Duration::from_secs(30));
        assert_eq!(manifest.total, Duration::from_secs(60));
        assert_eq!(manifest.max_redirects, 5);
        let installer = Limits::installer();
        assert_eq!(installer.timeouts.receive, Duration::from_secs(60));
        assert_eq!(installer.total, Duration::from_secs(1800));
        assert_eq!(installer.max_redirects, 5);
    }
}
