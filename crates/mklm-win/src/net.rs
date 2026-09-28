//! One HTTPS GET over WinHTTP (feature `net`, design m5b A.7, A.9): automatic proxy, TLS 1.2/1.3,
//! no cookies, no automatic redirects, no decompression, `WINHTTP_OPTION_AUTOLOGON_POLICY` = HIGH,
//! never `WinHttpSetCredentials` (SECURITY-13). Redirects, limits and URL rules are
//! `mklm_update::fetch`'s.
//!
//! WP-U implements it (every `unsafe` block with its `// SAFETY:` comment).

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::marker::PhantomData;

#[derive(Debug)]
pub struct HttpSession {
    /// Skeleton (M5b): WP-U keeps the `WinHttpOpen` handle here.
    _private: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpTimeouts {
    pub resolve_ms: i32,
    pub connect_ms: i32,
    pub send_ms: i32,
    pub receive_ms: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct HttpGet<'a> {
    /// false only in development builds: to 127.0.0.1, or to a `.invalid` host through an
    /// `open_named_proxy` session (F.3); refused otherwise.
    pub secure: bool,
    pub host: &'a str,
    pub port: u16,
    pub path_and_query: &'a str,
    pub accept: &'a str,
    pub timeouts: HttpTimeouts,
}

#[derive(Debug)]
pub struct HttpResponse<'s> {
    /// Skeleton (M5b): WP-U keeps the request handle here.
    session: PhantomData<&'s HttpSession>,
}

impl HttpSession {
    /// `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, TLS 1.2/1.3, no cookies, no automatic redirects,
    /// no decompression, `WINHTTP_OPTION_AUTOLOGON_POLICY` = HIGH; never `WinHttpSetCredentials`
    /// (design m5b A.7; SECURITY-13).
    pub fn open(user_agent: &str) -> Result<HttpSession, NetError> {
        Err(skeleton("WinHttpOpen (m5b skeleton)")) // Skeleton (M5b): WP-U
    }

    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn open_direct(user_agent: &str) -> Result<HttpSession, NetError> {
        Err(skeleton("WinHttpOpen (m5b skeleton)")) // Skeleton (M5b): WP-U
    }

    /// Tests only (F.3 proxy authentication; FIX-VERIFICATION-4): `WINHTTP_ACCESS_TYPE_NAMED_PROXY`
    /// with `proxy` = `127.0.0.1:<port>` (anything else refused), no bypass list, the other
    /// options as `open`, and the given autologon level (`Low` only for the control case).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn open_named_proxy(
        user_agent: &str,
        proxy: &str,
        autologon: AutologonLevel,
    ) -> Result<HttpSession, NetError> {
        Err(skeleton("WinHttpOpen (m5b skeleton)")) // Skeleton (M5b): WP-U
    }

    pub fn get(&self, request: &HttpGet<'_>) -> Result<HttpResponse<'_>, NetError> {
        Err(skeleton("WinHttpSendRequest (m5b skeleton)")) // Skeleton (M5b): WP-U
    }
}

#[cfg(all(debug_assertions, mklm_update_dev))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutologonLevel {
    /// `WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH` (what `open` always uses).
    High,
    /// `WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW`: the F.3 control case only.
    Low,
}

impl HttpResponse<'_> {
    pub fn status(&self) -> u16 {
        0 // Skeleton (M5b): WP-U
    }

    pub fn header(&self, name: &str) -> Option<String> {
        None // Skeleton (M5b): WP-U
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        Err(skeleton("WinHttpReadData (m5b skeleton)")) // Skeleton (M5b): WP-U
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetErrorKind {
    Timeout,
    NameNotResolved,
    CannotConnect,
    Tls,
    ProxyAuth,
    Cancelled,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{function} failed with WinHTTP error {code}")]
pub struct NetError {
    pub function: &'static str,
    pub code: u32,
    pub kind: NetErrorKind,
}

/// What the WP-0 skeleton returns (design m5b G.2): `ERROR_NOT_SUPPORTED`.
fn skeleton(function: &'static str) -> NetError {
    NetError {
        function,
        code: 50,
        kind: NetErrorKind::Other,
    }
}
