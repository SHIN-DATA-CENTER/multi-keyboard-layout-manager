//! [`Transport`] over WinHTTP (`mklm_win::net`, feature `winhttp`, Windows; design m5b A.7, A.9).
//!
//! A thin adapter: one [`HttpSession`] per transport (automatic proxy, autologon HIGH, no
//! credentials), each `get` one request on it. The fetch rules (redirects, limits, deadlines) are
//! [`crate::fetch`]'s.

use std::time::Duration;

use mklm_win::net::{HttpGet, HttpResponse, HttpSession, HttpTimeouts, NetError, NetErrorKind};

use crate::fetch::{Response, Timeouts, Transport, TransportError};
use crate::url::Url;

#[derive(Debug)]
pub struct WinHttpTransport {
    session: HttpSession,
}

impl WinHttpTransport {
    /// Automatic proxy, autologon HIGH, no credentials (design m5b A.7).
    pub fn new(user_agent: &str) -> Result<WinHttpTransport, TransportError> {
        HttpSession::open(user_agent)
            .map(|session| WinHttpTransport { session })
            .map_err(transport_error)
    }

    /// No proxy, for the loopback tests and the rehearsal.
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn new_without_proxy(user_agent: &str) -> Result<WinHttpTransport, TransportError> {
        HttpSession::open_direct(user_agent)
            .map(|session| WinHttpTransport { session })
            .map_err(transport_error)
    }
}

impl Transport for WinHttpTransport {
    fn get(
        &mut self,
        url: &Url,
        accept: &str,
        timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError> {
        let request = HttpGet {
            secure: url.is_https(),
            host: url.host(),
            port: url.port(),
            path_and_query: url.path_and_query(),
            accept,
            timeouts: HttpTimeouts {
                resolve_ms: millis(timeouts.resolve),
                connect_ms: millis(timeouts.connect),
                send_ms: millis(timeouts.send),
                receive_ms: millis(timeouts.receive),
            },
        };
        let response = self.session.get(&request).map_err(transport_error)?;
        Ok(Box::new(WinHttpResponse(response)))
    }
}

struct WinHttpResponse<'s>(HttpResponse<'s>);

impl Response for WinHttpResponse<'_> {
    fn status(&self) -> u16 {
        self.0.status()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.0.header(name)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        self.0.read(buf).map_err(transport_error)
    }
}

/// WinHTTP timeouts are milliseconds in an `i32`; 0 would mean "no timeout", so at least 1.
fn millis(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis())
        .unwrap_or(i32::MAX)
        .max(1)
}

fn transport_error(error: NetError) -> TransportError {
    match error.kind {
        NetErrorKind::Timeout => TransportError::Timeout,
        NetErrorKind::NameNotResolved => TransportError::NameNotResolved,
        NetErrorKind::CannotConnect => TransportError::CannotConnect,
        NetErrorKind::Tls => TransportError::Tls,
        NetErrorKind::ProxyAuth => TransportError::ProxyAuthRequired,
        NetErrorKind::Cancelled => TransportError::Cancelled,
        NetErrorKind::Other => TransportError::Other { code: error.code },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds() {
        assert_eq!(millis(Duration::from_secs(15)), 15_000);
        assert_eq!(millis(Duration::ZERO), 1);
        assert_eq!(millis(Duration::from_secs(u64::MAX)), i32::MAX);
    }

    #[test]
    fn error_mapping() {
        let error = |kind, code| NetError {
            function: "f",
            code,
            kind,
        };
        assert_eq!(
            transport_error(error(NetErrorKind::Timeout, 12002)),
            TransportError::Timeout
        );
        assert_eq!(
            transport_error(error(NetErrorKind::ProxyAuth, 0)),
            TransportError::ProxyAuthRequired
        );
        assert_eq!(
            transport_error(error(NetErrorKind::Other, 12030)),
            TransportError::Other { code: 12030 }
        );
    }
}
