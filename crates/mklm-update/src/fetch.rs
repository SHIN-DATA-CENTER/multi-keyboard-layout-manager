//! The fetch policy over a [`Transport`] (design m5b A.5 to A.8): redirects one at a time under a
//! [`UrlPolicy`](crate::url::UrlPolicy), size limits, deadlines, cancellation, SHA-256.
//!
//! - Only 301, 302, 303, 307 and 308 are redirects; each `Location` is resolved and checked
//!   against the endpoints' policy **before** anything connects to it (`RedirectNotAllowed`);
//!   at most `max_redirects` are followed (`TooManyRedirects`).
//! - 200 is the only success. 404 is `NotFound`, 429 (and a 403 that says it is a rate limit) is
//!   `RateLimited`, 407 is `Transport(ProxyAuthRequired)`, anything else (401 included: server
//!   authentication is never answered, SECURITY-13) is `HttpStatus`.
//! - A response with a `Content-Encoding` other than `identity` is refused (`UnexpectedEncoding`):
//!   no `Accept-Encoding` is ever sent.
//! - `Content-Length` over the limit (or, for the installer, other than the verified size) is
//!   refused before the body is read; the body is read up to one byte past the limit.
//! - The cancel flag and the whole-fetch deadline are checked before every request and every
//!   read; each request's timeouts are clamped to the time left. A timeout after the deadline is
//!   `DeadlineExceeded`.
//! - No retries (A.8).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::Version;
use crate::manifest::Sha256Stream;
use crate::url::{Endpoints, Url};
use crate::verify::{SelectedAsset, SignatureSlot};
use crate::{MANIFEST_NAME, MAX_MANIFEST_LEN, MAX_SIGNATURE_LEN};

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

/// The `Accept` of the manifest, the signatures and `SHA256SUMS` (A.7).
const ACCEPT_ANY: &str = "*/*";
/// The `Accept` of the installer (A.7).
const ACCEPT_INSTALLER: &str = "application/octet-stream";
/// One read from the transport.
const READ_LEN: usize = 64 * 1024;

/// One fetch: its deadline, the cancel flag and the policy of its endpoints.
struct Fetch<'a> {
    endpoints: &'a Endpoints,
    limits: &'a Limits,
    deadline: Instant,
    cancel: &'a AtomicBool,
}

/// How the body of a 200 response is checked.
enum Expect {
    /// At most this many bytes.
    AtMost(u64),
    /// Exactly this many bytes.
    Exactly(u64),
}

impl Fetch<'_> {
    fn new<'a>(endpoints: &'a Endpoints, limits: &'a Limits, cancel: &'a AtomicBool) -> Fetch<'a> {
        Fetch {
            endpoints,
            limits,
            deadline: Instant::now() + limits.total,
            cancel,
        }
    }

    /// Cancellation first, then the deadline.
    fn check(&self) -> Result<(), FetchError> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(FetchError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(FetchError::DeadlineExceeded);
        }
        Ok(())
    }

    /// A transport error, seen in the light of the deadline and the cancel flag.
    fn transport_error(&self, error: TransportError) -> FetchError {
        match error {
            TransportError::Cancelled => FetchError::Cancelled,
            _ if self.cancel.load(Ordering::SeqCst) => FetchError::Cancelled,
            TransportError::Timeout if Instant::now() >= self.deadline => {
                FetchError::DeadlineExceeded
            }
            error => FetchError::Transport(error),
        }
    }

    /// The limits' timeouts, none longer than the time left (at least 1 ms).
    fn timeouts(&self) -> Timeouts {
        let left = self
            .deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        let t = self.limits.timeouts;
        Timeouts {
            resolve: t.resolve.min(left),
            connect: t.connect.min(left),
            send: t.send.min(left),
            receive: t.receive.min(left),
        }
    }

    /// GET `url`, following the redirects the policy allows, then stream the 200 body into
    /// `sink` under `expect`. Returns the first redirect's target, if any.
    fn get(
        &self,
        transport: &mut dyn Transport,
        url: Url,
        accept: &str,
        expect: Expect,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), FetchError>,
    ) -> Result<Option<Url>, FetchError> {
        let mut url = url;
        let mut first_location = None;
        let mut redirects = 0u8;
        loop {
            self.check()?;
            let mut response = transport
                .get(&url, accept, &self.timeouts())
                .map_err(|error| self.transport_error(error))?;
            let status = response.status();
            match status {
                200 => {
                    self.read_body(response.as_mut(), expect, sink)?;
                    return Ok(first_location);
                }
                301 | 302 | 303 | 307 | 308 => {
                    if redirects >= self.limits.max_redirects {
                        return Err(FetchError::TooManyRedirects);
                    }
                    let location = response
                        .header("Location")
                        .ok_or(FetchError::MissingLocation)?;
                    let next = url
                        .resolve(&location, self.endpoints.policy())
                        .map_err(|_| FetchError::RedirectNotAllowed {
                            location: location.clone(),
                        })?;
                    drop(response);
                    redirects += 1;
                    if first_location.is_none() {
                        first_location = Some(next.clone());
                    }
                    url = next;
                }
                404 => return Err(FetchError::NotFound),
                407 => return Err(FetchError::Transport(TransportError::ProxyAuthRequired)),
                429 => return Err(FetchError::RateLimited { status }),
                403 if is_rate_limit(response.as_ref()) => {
                    return Err(FetchError::RateLimited { status });
                }
                _ => return Err(FetchError::HttpStatus { status }),
            }
        }
    }

    fn read_body(
        &self,
        response: &mut dyn Response,
        expect: Expect,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), FetchError>,
    ) -> Result<(), FetchError> {
        if response
            .header("Content-Encoding")
            .is_some_and(|encoding| !encoding.trim().eq_ignore_ascii_case("identity"))
        {
            return Err(FetchError::UnexpectedEncoding);
        }
        let content_length = response
            .header("Content-Length")
            .and_then(|text| parse_decimal(text.trim()));
        let limit = match expect {
            Expect::AtMost(limit) => {
                if content_length.is_some_and(|length| length > limit) {
                    return Err(FetchError::TooLarge { limit });
                }
                limit
            }
            Expect::Exactly(size) => {
                if let Some(length) = content_length.filter(|&length| length != size) {
                    return Err(FetchError::SizeMismatch {
                        expected: size,
                        received: length,
                    });
                }
                size
            }
        };
        let mut buf = vec![0u8; READ_LEN];
        let mut total = 0u64;
        loop {
            self.check()?;
            // Never more than one byte past the limit.
            let want = usize::try_from(
                limit
                    .saturating_add(1)
                    .saturating_sub(total)
                    .min(READ_LEN as u64),
            )
            .unwrap_or(READ_LEN);
            let read = response
                .read(&mut buf[..want])
                .map_err(|error| self.transport_error(error))?;
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > limit {
                return Err(match expect {
                    Expect::AtMost(limit) => FetchError::TooLarge { limit },
                    Expect::Exactly(size) => FetchError::SizeMismatch {
                        expected: size,
                        received: total,
                    },
                });
            }
            sink(&buf[..read])?;
        }
        if let Expect::Exactly(size) = expect
            && total != size
        {
            return Err(FetchError::SizeMismatch {
                expected: size,
                received: total,
            });
        }
        Ok(())
    }

    /// A small file into memory.
    fn get_bytes(
        &self,
        transport: &mut dyn Transport,
        url: Url,
        max_len: u64,
    ) -> Result<(Vec<u8>, Option<Url>), FetchError> {
        let mut body = Vec::new();
        let location = self.get(
            transport,
            url,
            ACCEPT_ANY,
            Expect::AtMost(max_len),
            &mut |bytes| {
                body.extend_from_slice(bytes);
                Ok(())
            },
        )?;
        Ok((body, location))
    }
}

/// GitHub answers some rate limits with 403 and one of these headers.
fn is_rate_limit(response: &dyn Response) -> bool {
    response.header("Retry-After").is_some()
        || response
            .header("X-RateLimit-Remaining")
            .is_some_and(|remaining| remaining.trim() == "0")
}

/// Decimal digits only.
fn parse_decimal(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// latest.json, then its main signature (from the same tag when known). Design m5b A.5–A.8.
pub fn fetch_manifest(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<FetchedManifest, FetchError> {
    // One deadline for both files (A.8).
    let fetch = Fetch::new(endpoints, limits, cancel);
    let (manifest, location) =
        fetch.get_bytes(transport, endpoints.manifest_url(), MAX_MANIFEST_LEN as u64)?;
    let tag = location.and_then(|location| endpoints.tag_from_location(&location, MANIFEST_NAME));
    let (signature, _) = fetch.get_bytes(
        transport,
        endpoints.signature_url(tag.as_deref(), SignatureSlot::Main),
        MAX_SIGNATURE_LEN as u64,
    )?;
    Ok(FetchedManifest {
        manifest,
        signature,
        tag,
    })
}

/// The alternate signature of the same tag; `Ok(None)` on 404 (design m5b A.5).
pub fn fetch_alt_signature(
    transport: &mut dyn Transport,
    endpoints: &Endpoints,
    tag: Option<&str>,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Option<Vec<u8>>, FetchError> {
    let fetch = Fetch::new(endpoints, limits, cancel);
    match fetch.get_bytes(
        transport,
        endpoints.signature_url(tag, SignatureSlot::Alt),
        MAX_SIGNATURE_LEN as u64,
    ) {
        Ok((signature, _)) => Ok(Some(signature)),
        Err(FetchError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
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
    let fetch = Fetch::new(endpoints, limits, cancel);
    let (body, location) = fetch.get_bytes(transport, endpoints.latest_file_url(name), max_len)?;
    let tag = location.and_then(|location| endpoints.tag_from_location(&location, name));
    Ok((body, tag))
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
    let fetch = Fetch::new(endpoints, limits, cancel);
    let mut hash = Sha256Stream::new();
    let mut received = 0u64;
    fetch.get(
        transport,
        endpoints.asset_url(version, &asset.name),
        ACCEPT_INSTALLER,
        Expect::Exactly(asset.size),
        &mut |bytes| {
            sink.write_all(bytes).map_err(|error| FetchError::Sink {
                detail: error.to_string(),
            })?;
            hash.update(bytes);
            received += bytes.len() as u64;
            progress(received);
            Ok(())
        },
    )?;
    if hash.finish() != asset.sha256 {
        return Err(FetchError::HashMismatch);
    }
    sink.flush().map_err(|error| FetchError::Sink {
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::manifest::{Arch, Sha256Digest};

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

    /// One scripted answer of [`FakeTransport`].
    #[derive(Debug, Clone)]
    struct Answer {
        status: u16,
        headers: Vec<(&'static str, String)>,
        /// Delivered in these pieces.
        body: Vec<Vec<u8>>,
        /// Returned by `get` instead of a response.
        error: Option<TransportError>,
        /// Returned by the read after the body pieces.
        read_error: Option<TransportError>,
    }

    fn ok(body: &[u8]) -> Answer {
        Answer {
            status: 200,
            headers: vec![("Content-Length", body.len().to_string())],
            body: vec![body.to_vec()],
            error: None,
            read_error: None,
        }
    }

    fn status(status: u16) -> Answer {
        Answer {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            error: None,
            read_error: None,
        }
    }

    fn redirect(location: &str) -> Answer {
        Answer {
            headers: vec![("Location", location.to_string())],
            ..status(302)
        }
    }

    /// Answers in order; records every request URL and its accept.
    #[derive(Debug, Default)]
    struct FakeTransport {
        answers: VecDeque<Answer>,
        requests: Vec<(String, String)>,
        timeouts: Vec<Timeouts>,
    }

    impl FakeTransport {
        fn new(answers: impl IntoIterator<Item = Answer>) -> FakeTransport {
            FakeTransport {
                answers: answers.into_iter().collect(),
                ..FakeTransport::default()
            }
        }

        fn urls(&self) -> Vec<&str> {
            self.requests.iter().map(|(url, _)| url.as_str()).collect()
        }
    }

    struct FakeResponse {
        answer: Answer,
    }

    impl Response for FakeResponse {
        fn status(&self) -> u16 {
            self.answer.status
        }

        fn header(&self, name: &str) -> Option<String> {
            self.answer
                .headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        }

        fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
            let Some(piece) = self.answer.body.first_mut() else {
                return match self.answer.read_error.take() {
                    Some(error) => Err(error),
                    None => Ok(0),
                };
            };
            let n = piece.len().min(buf.len());
            buf[..n].copy_from_slice(&piece[..n]);
            piece.drain(..n);
            if piece.is_empty() {
                self.answer.body.remove(0);
            }
            Ok(n)
        }
    }

    impl Transport for FakeTransport {
        fn get(
            &mut self,
            url: &Url,
            accept: &str,
            timeouts: &Timeouts,
        ) -> Result<Box<dyn Response + '_>, TransportError> {
            self.requests
                .push((url.as_str().to_string(), accept.to_string()));
            self.timeouts.push(*timeouts);
            let answer = self
                .answers
                .pop_front()
                .expect("an answer for every request");
            if let Some(error) = answer.error {
                return Err(error);
            }
            Ok(Box::new(FakeResponse { answer }))
        }
    }

    const REPO: &str = "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager";

    fn not_cancelled() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn manifest_and_signature_of_the_same_tag() {
        let mut transport = FakeTransport::new([
            redirect(&format!("{REPO}/releases/download/v0.2.1/latest.json")),
            redirect("https://release-assets.githubusercontent.com/asset/1?sig=a%2Bb&se=1"),
            ok(b"{}"),
            redirect("https://release-assets.githubusercontent.com/asset/2"),
            ok(b"sig"),
        ]);
        let fetched = fetch_manifest(
            &mut transport,
            &Endpoints::production(),
            &Limits::manifest(),
            &not_cancelled(),
        )
        .unwrap();
        assert_eq!(
            fetched,
            FetchedManifest {
                manifest: b"{}".to_vec(),
                signature: b"sig".to_vec(),
                tag: Some("v0.2.1".to_string()),
            }
        );
        assert_eq!(
            transport.urls(),
            [
                format!("{REPO}/releases/latest/download/latest.json").as_str(),
                &format!("{REPO}/releases/download/v0.2.1/latest.json"),
                "https://release-assets.githubusercontent.com/asset/1?sig=a%2Bb&se=1",
                &format!("{REPO}/releases/download/v0.2.1/latest.json.minisig"),
                "https://release-assets.githubusercontent.com/asset/2",
            ]
        );
        assert!(transport.requests.iter().all(|(_, accept)| accept == "*/*"));
    }

    #[test]
    fn without_a_tag_the_signature_comes_from_latest() {
        // A relative path on the same host, of another shape: no tag.
        let mut transport = FakeTransport::new([
            redirect("/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/assets/latest.json"),
            ok(b"{}"),
            ok(b"sig"),
        ]);
        let fetched = fetch_manifest(
            &mut transport,
            &Endpoints::production(),
            &Limits::manifest(),
            &not_cancelled(),
        )
        .unwrap();
        assert_eq!(fetched.tag, None);
        assert_eq!(
            transport.urls()[2],
            format!("{REPO}/releases/latest/download/latest.json.minisig")
        );
        // No redirect at all: no tag either.
        let mut transport = FakeTransport::new([ok(b"{}"), ok(b"sig")]);
        let fetched = fetch_manifest(
            &mut transport,
            &Endpoints::production(),
            &Limits::manifest(),
            &not_cancelled(),
        )
        .unwrap();
        assert_eq!(fetched.tag, None);
    }

    #[test]
    fn redirects_are_checked_before_connecting() {
        for location in [
            "http://github.com/x",
            "https://evil.example/x",
            "https://github.com:444/x",
            "https://user@github.com/x",
            "relative/path",
            "//evil.example/x",
        ] {
            let mut transport = FakeTransport::new([redirect(location)]);
            let result = fetch_latest_file(
                &mut transport,
                &Endpoints::production(),
                "SHA256SUMS",
                1024,
                &Limits::manifest(),
                &not_cancelled(),
            );
            assert_eq!(
                result,
                Err(FetchError::RedirectNotAllowed {
                    location: location.to_string()
                }),
                "{location}"
            );
            // Only the first request was made.
            assert_eq!(transport.requests.len(), 1, "{location}");
        }
        let mut transport = FakeTransport::new([status(301)]);
        assert_eq!(
            fetch_latest_file(
                &mut transport,
                &Endpoints::production(),
                "SHA256SUMS",
                1024,
                &Limits::manifest(),
                &not_cancelled(),
            ),
            Err(FetchError::MissingLocation)
        );
    }

    #[test]
    fn five_redirects_are_followed_and_the_sixth_is_refused() {
        let hop = |n: usize| redirect(&format!("https://github.com/hop/{n}"));
        let mut five: Vec<Answer> = (0..5).map(hop).collect();
        five.push(ok(b"x"));
        let mut transport = FakeTransport::new(five);
        assert!(
            fetch_latest_file(
                &mut transport,
                &Endpoints::production(),
                "SHA256SUMS",
                1024,
                &Limits::manifest(),
                &not_cancelled(),
            )
            .is_ok()
        );
        let mut transport = FakeTransport::new((0..6).map(hop));
        assert_eq!(
            fetch_latest_file(
                &mut transport,
                &Endpoints::production(),
                "SHA256SUMS",
                1024,
                &Limits::manifest(),
                &not_cancelled(),
            ),
            Err(FetchError::TooManyRedirects)
        );
        assert_eq!(transport.requests.len(), 6);
        for code in [301, 303, 307, 308] {
            let mut transport = FakeTransport::new([
                Answer {
                    status: code,
                    ..hop(1)
                },
                ok(b"x"),
            ]);
            assert!(
                fetch_latest_file(
                    &mut transport,
                    &Endpoints::production(),
                    "SHA256SUMS",
                    1024,
                    &Limits::manifest(),
                    &not_cancelled(),
                )
                .is_ok(),
                "{code}"
            );
        }
    }

    #[test]
    fn status_codes() {
        let limited_403 = Answer {
            headers: vec![("X-RateLimit-Remaining", "0".to_string())],
            ..status(403)
        };
        for (answer, expected) in [
            (status(404), FetchError::NotFound),
            (status(429), FetchError::RateLimited { status: 429 }),
            (limited_403, FetchError::RateLimited { status: 403 }),
            (status(403), FetchError::HttpStatus { status: 403 }),
            (status(500), FetchError::HttpStatus { status: 500 }),
            (status(401), FetchError::HttpStatus { status: 401 }),
            (status(204), FetchError::HttpStatus { status: 204 }),
            (status(300), FetchError::HttpStatus { status: 300 }),
            (
                status(407),
                FetchError::Transport(TransportError::ProxyAuthRequired),
            ),
        ] {
            let mut transport = FakeTransport::new([answer]);
            assert_eq!(
                fetch_latest_file(
                    &mut transport,
                    &Endpoints::production(),
                    "SHA256SUMS",
                    1024,
                    &Limits::manifest(),
                    &not_cancelled(),
                ),
                Err(expected.clone()),
                "{expected:?}"
            );
        }
    }

    #[test]
    fn the_alternate_signature_may_be_missing() {
        let mut transport = FakeTransport::new([status(404)]);
        assert_eq!(
            fetch_alt_signature(
                &mut transport,
                &Endpoints::production(),
                Some("v0.2.1"),
                &Limits::manifest(),
                &not_cancelled()
            ),
            Ok(None)
        );
        assert_eq!(
            transport.urls(),
            [format!("{REPO}/releases/download/v0.2.1/latest.json.alt.minisig").as_str()]
        );
        let mut transport = FakeTransport::new([ok(b"alt")]);
        assert_eq!(
            fetch_alt_signature(
                &mut transport,
                &Endpoints::production(),
                None,
                &Limits::manifest(),
                &not_cancelled()
            ),
            Ok(Some(b"alt".to_vec()))
        );
        assert_eq!(
            transport.urls(),
            [format!("{REPO}/releases/latest/download/latest.json.alt.minisig").as_str()]
        );
        let mut transport = FakeTransport::new([status(500)]);
        assert_eq!(
            fetch_alt_signature(
                &mut transport,
                &Endpoints::production(),
                None,
                &Limits::manifest(),
                &not_cancelled()
            ),
            Err(FetchError::HttpStatus { status: 500 })
        );
    }

    #[test]
    fn size_limits_before_and_while_reading() {
        let fetch = |answer: Answer| {
            let mut transport = FakeTransport::new([answer]);
            fetch_latest_file(
                &mut transport,
                &Endpoints::production(),
                "SHA256SUMS",
                10,
                &Limits::manifest(),
                &not_cancelled(),
            )
        };
        // Content-Length over the limit: nothing is read (the body would be fine).
        let announced = Answer {
            headers: vec![("Content-Length", "11".to_string())],
            ..ok(b"short")
        };
        assert_eq!(fetch(announced), Err(FetchError::TooLarge { limit: 10 }));
        // No Content-Length (chunked): refused at limit + 1 bytes.
        let chunked = Answer {
            headers: Vec::new(),
            body: vec![b"123456".to_vec(), b"78901".to_vec()],
            ..ok(b"")
        };
        assert_eq!(fetch(chunked), Err(FetchError::TooLarge { limit: 10 }));
        let exact = Answer {
            headers: Vec::new(),
            body: vec![b"12345".to_vec(), b"67890".to_vec()],
            ..ok(b"")
        };
        assert_eq!(fetch(exact).unwrap().0, b"1234567890");
        // A compressed answer.
        let gzip = Answer {
            headers: vec![("Content-Encoding", "gzip".to_string())],
            ..ok(b"x")
        };
        assert_eq!(fetch(gzip), Err(FetchError::UnexpectedEncoding));
        let identity = Answer {
            headers: vec![("content-encoding", "identity".to_string())],
            ..ok(b"x")
        };
        assert!(fetch(identity).is_ok());
    }

    fn asset_of(content: &[u8]) -> SelectedAsset {
        SelectedAsset {
            arch: Arch::X64,
            name: "MKLM-Setup-0.2.1-x64.exe".to_string(),
            size: content.len() as u64,
            sha256: Sha256Digest::of(content),
        }
    }

    fn download(
        answer: Answer,
        asset: &SelectedAsset,
    ) -> (Result<(), FetchError>, Vec<u8>, Vec<u64>) {
        let mut transport = FakeTransport::new([answer]);
        let mut sink = Vec::new();
        let mut progress = Vec::new();
        let result = download_asset(
            &mut transport,
            &Endpoints::production(),
            &Version::new(0, 2, 1),
            asset,
            &Limits::installer(),
            &mut sink,
            &mut |bytes| progress.push(bytes),
            &not_cancelled(),
        );
        assert_eq!(
            transport.requests,
            [(
                format!("{REPO}/releases/download/v0.2.1/MKLM-Setup-0.2.1-x64.exe"),
                "application/octet-stream".to_string()
            )]
        );
        (result, sink, progress)
    }

    #[test]
    fn installer_downloads_check_size_and_hash() {
        let content: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
        let asset = asset_of(&content);
        let (result, sink, progress) = download(ok(&content), &asset);
        assert_eq!(result, Ok(()));
        assert_eq!(sink, content);
        assert_eq!(progress.last(), Some(&200_000));
        assert!(progress.windows(2).all(|w| w[0] < w[1]));
        // Content-Length other than the size: refused unread.
        let (result, sink, _) = download(ok(&content[..10]), &asset);
        assert_eq!(
            result,
            Err(FetchError::SizeMismatch {
                expected: 200_000,
                received: 10
            })
        );
        assert!(sink.is_empty());
        // Chunked, short and long.
        let unannounced = |body: &[u8]| Answer {
            headers: Vec::new(),
            ..ok(body)
        };
        assert_eq!(
            download(unannounced(&content[..199_999]), &asset).0,
            Err(FetchError::SizeMismatch {
                expected: 200_000,
                received: 199_999
            })
        );
        let mut longer = content.clone();
        longer.push(0);
        assert_eq!(
            download(unannounced(&longer), &asset).0,
            Err(FetchError::SizeMismatch {
                expected: 200_000,
                received: 200_001
            })
        );
        // Same size, other content.
        let mut other = content.clone();
        other[1234] ^= 0xFF;
        assert_eq!(
            download(ok(&other), &asset).0,
            Err(FetchError::HashMismatch)
        );
    }

    #[test]
    fn sink_errors_and_transport_errors() {
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let content = b"installer".to_vec();
        let asset = asset_of(&content);
        let mut transport = FakeTransport::new([ok(&content)]);
        let result = download_asset(
            &mut transport,
            &Endpoints::production(),
            &Version::new(0, 2, 1),
            &asset,
            &Limits::installer(),
            &mut Broken,
            &mut |_| {},
            &not_cancelled(),
        );
        assert!(matches!(result, Err(FetchError::Sink { .. })), "{result:?}");

        for error in [
            TransportError::Timeout,
            TransportError::NameNotResolved,
            TransportError::CannotConnect,
            TransportError::Tls,
            TransportError::ProxyAuthRequired,
            TransportError::Other { code: 12030 },
        ] {
            let mut transport = FakeTransport::new([Answer {
                error: Some(error),
                ..status(0)
            }]);
            assert_eq!(
                fetch_latest_file(
                    &mut transport,
                    &Endpoints::production(),
                    "x",
                    10,
                    &Limits::manifest(),
                    &not_cancelled()
                ),
                Err(FetchError::Transport(error))
            );
            let mut transport = FakeTransport::new([Answer {
                read_error: Some(error),
                ..ok(b"12")
            }]);
            assert_eq!(
                fetch_latest_file(
                    &mut transport,
                    &Endpoints::production(),
                    "x",
                    10,
                    &Limits::manifest(),
                    &not_cancelled()
                ),
                Err(FetchError::Transport(error))
            );
        }
        let mut transport = FakeTransport::new([Answer {
            error: Some(TransportError::Cancelled),
            ..status(0)
        }]);
        assert_eq!(
            fetch_latest_file(
                &mut transport,
                &Endpoints::production(),
                "x",
                10,
                &Limits::manifest(),
                &not_cancelled()
            ),
            Err(FetchError::Cancelled)
        );
    }

    #[test]
    fn cancellation_and_deadline() {
        // Cancelled before the first request: nothing is requested.
        let cancelled = AtomicBool::new(true);
        let mut transport = FakeTransport::new([]);
        assert_eq!(
            fetch_manifest(
                &mut transport,
                &Endpoints::production(),
                &Limits::manifest(),
                &cancelled
            ),
            Err(FetchError::Cancelled)
        );
        assert!(transport.requests.is_empty());
        // Cancelled while downloading: the next read does not happen.
        let content = vec![7u8; 3 * READ_LEN];
        let asset = asset_of(&content);
        let cancel = AtomicBool::new(false);
        let mut transport = FakeTransport::new([Answer {
            body: content.chunks(READ_LEN).map(<[u8]>::to_vec).collect(),
            ..ok(&content)
        }]);
        let mut sink = Vec::new();
        let result = download_asset(
            &mut transport,
            &Endpoints::production(),
            &Version::new(0, 2, 1),
            &asset,
            &Limits::installer(),
            &mut sink,
            &mut |_| cancel.store(true, Ordering::SeqCst),
            &cancel,
        );
        assert_eq!(result, Err(FetchError::Cancelled));
        assert_eq!(sink.len(), READ_LEN);
        // A deadline already over: nothing is requested.
        let limits = Limits {
            total: Duration::ZERO,
            ..Limits::manifest()
        };
        let mut transport = FakeTransport::new([]);
        assert_eq!(
            fetch_manifest(
                &mut transport,
                &Endpoints::production(),
                &limits,
                &not_cancelled()
            ),
            Err(FetchError::DeadlineExceeded)
        );
        // Timeouts never exceed the time left.
        let limits = Limits {
            total: Duration::from_secs(5),
            ..Limits::manifest()
        };
        let mut transport = FakeTransport::new([ok(b"x")]);
        fetch_latest_file(
            &mut transport,
            &Endpoints::production(),
            "x",
            10,
            &limits,
            &not_cancelled(),
        )
        .unwrap();
        let used = transport.timeouts[0];
        assert!(used.receive <= Duration::from_secs(5) && used.connect <= Duration::from_secs(5));
    }
}
