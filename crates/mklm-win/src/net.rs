//! One HTTPS GET over WinHTTP (feature `net`, design m5b A.7, A.9): automatic proxy, TLS 1.2/1.3,
//! no cookies, no automatic redirects, no decompression, `WINHTTP_OPTION_AUTOLOGON_POLICY` = HIGH,
//! never `WinHttpSetCredentials` (SECURITY-13). Redirects, limits and URL rules are
//! `mklm_update::fetch`'s.
//!
//! Synchronous WinHTTP (no `WINHTTP_FLAG_ASYNC`, so no `WINHTTP_FLAG_SECURE_DEFAULTS`, which
//! forces asynchronous mode): the caller runs it on its own thread. The functions used are the ten
//! of design m5b A.9: `WinHttpOpen`, `WinHttpSetOption`, `WinHttpSetTimeouts`, `WinHttpConnect`,
//! `WinHttpOpenRequest`, `WinHttpSendRequest`, `WinHttpReceiveResponse`, `WinHttpQueryHeaders`,
//! `WinHttpReadData`, `WinHttpCloseHandle`.
//!
//! What goes on the wire: `GET`, the path exactly as given (no escaping), `User-Agent` (from
//! `WinHttpOpen`), `Accept`, and WinHTTP's own `Host` and `Connection`. No `Accept-Encoding`, no
//! cookies, no `Authorization`: a 401 or a 407 is returned as the response's status, never
//! answered (J-8 (a)). Two options keep WinHTTP from answering NTLM or Negotiate with the user's
//! Windows credentials, to a server or to a proxy:
//! - `WINHTTP_OPTION_AUTOLOGON_POLICY` = HIGH (design m5b A.7);
//! - `WINHTTP_DISABLE_AUTHENTICATION` among the disabled features: WinHTTP's automatic
//!   authentication is off. HIGH alone does not apply to a server named by an IP address
//!   (Microsoft Learn, `WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH`); the loopback test of design m5b
//!   F.3 saw WinHTTP send an NTLM negotiate message to `127.0.0.1` under HIGH alone. The product's
//!   URL policy never names a server by address, and this option closes the gap anyway.
//!
//! Plain http exists in development builds only (`cfg(all(debug_assertions, mklm_update_dev))`):
//! to 127.0.0.1, or to a `.invalid` name through an [`HttpSession::open_named_proxy`] session
//! (the proxy authentication test of design m5b F.3).

use std::ffi::c_void;
use std::marker::PhantomData;

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED,
    ERROR_WINHTTP_NAME_NOT_RESOLVED, ERROR_WINHTTP_OPERATION_CANCELLED,
    ERROR_WINHTTP_SECURE_CERT_CN_INVALID, ERROR_WINHTTP_SECURE_CERT_DATE_INVALID,
    ERROR_WINHTTP_SECURE_CERT_REV_FAILED, ERROR_WINHTTP_SECURE_CERT_REVOKED,
    ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE, ERROR_WINHTTP_SECURE_CHANNEL_ERROR,
    ERROR_WINHTTP_SECURE_FAILURE, ERROR_WINHTTP_SECURE_FAILURE_PROXY,
    ERROR_WINHTTP_SECURE_INVALID_CA, ERROR_WINHTTP_SECURE_INVALID_CERT, ERROR_WINHTTP_TIMEOUT,
    WINHTTP_ACCESS_TYPE, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, WINHTTP_DISABLE_AUTHENTICATION, WINHTTP_DISABLE_COOKIES,
    WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_ESCAPE_DISABLE, WINHTTP_FLAG_ESCAPE_DISABLE_QUERY,
    WINHTTP_FLAG_SECURE, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
    WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RAW_HEADERS_CRLF,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::core::{PCWSTR, w};

/// `ERROR_INVALID_PARAMETER`: a request this module refuses before calling WinHTTP.
const ERROR_INVALID_PARAMETER: u32 = 87;

/// An open WinHTTP handle (session, connection or request), closed on drop.
#[derive(Debug)]
struct Handle(*mut c_void);

// SAFETY: a WinHTTP handle (HINTERNET) is not bound to the thread that created it; synchronous
// WinHTTP may be called on it from any thread (Microsoft Learn, "WinHTTP Sessions Overview").
// Moving the owner between threads moves the only way to use or close it.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: `self.0` is a handle WinHTTP returned and nothing else closes; after this call
        // it is never used again.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

#[derive(Debug)]
pub struct HttpSession {
    handle: Handle,
    /// `WINHTTP_OPTION_AUTOLOGON_POLICY` of every request (HIGH except the F.3 control case).
    autologon: u32,
    /// `WINHTTP_DISABLE_AUTHENTICATION` in every request's disabled features (0 only in the F.3
    /// control case).
    disable_authentication: u32,
    /// An `open_named_proxy` session: plain http to a `.invalid` name is allowed (F.3).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    named_proxy: bool,
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
    /// Declared before `_connection`: the request handle closes first.
    request: Handle,
    _connection: Handle,
    status: u16,
    /// The response's header lines, in order (names as sent).
    headers: Vec<(String, String)>,
    session: PhantomData<&'s HttpSession>,
}

impl HttpSession {
    /// `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, TLS 1.2/1.3, no cookies, no automatic redirects,
    /// no decompression, `WINHTTP_OPTION_AUTOLOGON_POLICY` = HIGH; never `WinHttpSetCredentials`
    /// (design m5b A.7; SECURITY-13). Also `WINHTTP_DISABLE_AUTHENTICATION` (module docs).
    pub fn open(user_agent: &str) -> Result<HttpSession, NetError> {
        let handle = open_handle(user_agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, None)?;
        Ok(HttpSession {
            handle,
            autologon: WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
            disable_authentication: WINHTTP_DISABLE_AUTHENTICATION,
            #[cfg(all(debug_assertions, mklm_update_dev))]
            named_proxy: false,
        })
    }

    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn open_direct(user_agent: &str) -> Result<HttpSession, NetError> {
        let handle = open_handle(
            user_agent,
            windows::Win32::Networking::WinHttp::WINHTTP_ACCESS_TYPE_NO_PROXY,
            None,
        )?;
        Ok(HttpSession {
            handle,
            autologon: WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
            disable_authentication: WINHTTP_DISABLE_AUTHENTICATION,
            named_proxy: false,
        })
    }

    /// Tests only (F.3 proxy authentication; FIX-VERIFICATION-4): `WINHTTP_ACCESS_TYPE_NAMED_PROXY`
    /// with `proxy` = `127.0.0.1:<port>` (anything else refused), no bypass list, the other
    /// options as `open`, and the given autologon level (`Low` only for the control case, which
    /// also leaves WinHTTP's automatic authentication on).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn open_named_proxy(
        user_agent: &str,
        proxy: &str,
        autologon: AutologonLevel,
    ) -> Result<HttpSession, NetError> {
        let port_ok = proxy.strip_prefix("127.0.0.1:").is_some_and(|port| {
            !port.is_empty()
                && !port.starts_with('0')
                && port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u16>().is_ok()
        });
        if !port_ok {
            return Err(refused(
                "WinHttpOpen (named proxy other than 127.0.0.1:<port>)",
            ));
        }
        let handle = open_handle(
            user_agent,
            windows::Win32::Networking::WinHttp::WINHTTP_ACCESS_TYPE_NAMED_PROXY,
            Some(proxy),
        )?;
        let (autologon, disable_authentication) = match autologon {
            // The product's options.
            AutologonLevel::High => (
                WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
                WINHTTP_DISABLE_AUTHENTICATION,
            ),
            // The control case: WinHTTP's automatic authentication on, autologon LOW, to show
            // that this machine would send credentials without the product's options.
            AutologonLevel::Low => (
                windows::Win32::Networking::WinHttp::WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW,
                0,
            ),
        };
        Ok(HttpSession {
            handle,
            autologon,
            disable_authentication,
            named_proxy: true,
        })
    }

    pub fn get(&self, request: &HttpGet<'_>) -> Result<HttpResponse<'_>, NetError> {
        self.check_plain(request)?;
        let host = wide(request.host, "WinHttpConnect")?;
        let path = wide(request.path_and_query, "WinHttpOpenRequest")?;
        let accept = wide(request.accept, "WinHttpOpenRequest")?;
        if request.host.is_empty() || !request.path_and_query.starts_with('/') {
            return Err(refused("WinHttpOpenRequest"));
        }

        // SAFETY: the session handle is open; `host` is NUL-terminated and outlives the call.
        let connection =
            unsafe { WinHttpConnect(self.handle.0, PCWSTR(host.as_ptr()), request.port, 0) };
        if connection.is_null() {
            return Err(last_error("WinHttpConnect"));
        }
        let connection = Handle(connection);

        let accept_types = [PCWSTR(accept.as_ptr()), PCWSTR::null()];
        let mut flags = WINHTTP_FLAG_ESCAPE_DISABLE.0 | WINHTTP_FLAG_ESCAPE_DISABLE_QUERY.0;
        if request.secure {
            flags |= WINHTTP_FLAG_SECURE.0;
        }
        // SAFETY: the connection handle is open; the verb, `path` and every `accept_types` entry
        // are NUL-terminated and outlive the call; `accept_types` ends with a null entry; a null
        // version means HTTP/1.1 and a null referrer sends none.
        let raw = unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                accept_types.as_ptr(),
                WINHTTP_OPEN_REQUEST_FLAGS(flags),
            )
        };
        if raw.is_null() {
            return Err(last_error("WinHttpOpenRequest"));
        }
        let request_handle = Handle(raw);

        set_u32(
            &request_handle,
            WINHTTP_OPTION_DISABLE_FEATURE,
            WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | self.disable_authentication,
            "WinHttpSetOption(WINHTTP_OPTION_DISABLE_FEATURE)",
        )?;
        set_u32(
            &request_handle,
            WINHTTP_OPTION_AUTOLOGON_POLICY,
            self.autologon,
            "WinHttpSetOption(WINHTTP_OPTION_AUTOLOGON_POLICY)",
        )?;
        let t = request.timeouts;
        // SAFETY: the request handle is open.
        unsafe {
            WinHttpSetTimeouts(
                request_handle.0,
                t.resolve_ms,
                t.connect_ms,
                t.send_ms,
                t.receive_ms,
            )
        }
        .map_err(|error| from_error("WinHttpSetTimeouts", &error))?;
        // SAFETY: the request handle is open; no extra headers and no body.
        unsafe { WinHttpSendRequest(request_handle.0, None, None, 0, 0, 0) }
            .map_err(|error| from_error("WinHttpSendRequest", &error))?;
        // SAFETY: the request handle is open and its request was sent; the reserved argument is
        // null.
        unsafe { WinHttpReceiveResponse(request_handle.0, std::ptr::null_mut()) }
            .map_err(|error| from_error("WinHttpReceiveResponse", &error))?;

        let status = query_status(&request_handle)?;
        let headers = parse_raw_headers(&query_raw_headers(&request_handle)?);
        Ok(HttpResponse {
            request: request_handle,
            _connection: connection,
            status,
            headers,
            session: PhantomData,
        })
    }

    /// Plain http: development builds only, to 127.0.0.1, or to a `.invalid` name through a named
    /// proxy session.
    fn check_plain(&self, request: &HttpGet<'_>) -> Result<(), NetError> {
        if request.secure {
            return Ok(());
        }
        #[cfg(all(debug_assertions, mklm_update_dev))]
        {
            let loopback = request.host == "127.0.0.1";
            let invalid_through_proxy = self.named_proxy
                && request
                    .host
                    .strip_suffix(".invalid")
                    .is_some_and(|name| !name.is_empty());
            if loopback || invalid_through_proxy {
                return Ok(());
            }
        }
        Err(refused("WinHttpOpenRequest (plain http)"))
    }
}

#[cfg(all(debug_assertions, mklm_update_dev))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutologonLevel {
    /// `WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH` (what `open` always uses), with
    /// `WINHTTP_DISABLE_AUTHENTICATION` as in `open`.
    High,
    /// `WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW` with WinHTTP's automatic authentication on: the F.3
    /// control case only.
    Low,
}

impl HttpResponse<'_> {
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Case-insensitive; several lines of the same name are joined with `", "` (RFC 9110 5.3).
    pub fn header(&self, name: &str) -> Option<String> {
        let values: Vec<&str> = self
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let mut read = 0u32;
        // SAFETY: the request handle is open and its response was received; `buf` is writable
        // for `len` bytes; `read` receives the count.
        unsafe { WinHttpReadData(self.request.0, buf.as_mut_ptr().cast(), len, &mut read) }
            .map_err(|error| from_error("WinHttpReadData", &error))?;
        Ok(read as usize)
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

/// The kind of a Win32 / WinHTTP error code.
fn kind_of(code: u32) -> NetErrorKind {
    match code {
        ERROR_WINHTTP_TIMEOUT => NetErrorKind::Timeout,
        ERROR_WINHTTP_NAME_NOT_RESOLVED => NetErrorKind::NameNotResolved,
        ERROR_WINHTTP_CANNOT_CONNECT => NetErrorKind::CannotConnect,
        ERROR_WINHTTP_OPERATION_CANCELLED => NetErrorKind::Cancelled,
        ERROR_WINHTTP_SECURE_FAILURE
        | ERROR_WINHTTP_SECURE_CHANNEL_ERROR
        | ERROR_WINHTTP_SECURE_INVALID_CA
        | ERROR_WINHTTP_SECURE_INVALID_CERT
        | ERROR_WINHTTP_SECURE_CERT_CN_INVALID
        | ERROR_WINHTTP_SECURE_CERT_DATE_INVALID
        | ERROR_WINHTTP_SECURE_CERT_REV_FAILED
        | ERROR_WINHTTP_SECURE_CERT_REVOKED
        | ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE
        | ERROR_WINHTTP_SECURE_FAILURE_PROXY
        | ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED => NetErrorKind::Tls,
        _ => NetErrorKind::Other,
    }
}

fn net_error(function: &'static str, code: u32) -> NetError {
    NetError {
        function,
        code,
        kind: kind_of(code),
    }
}

/// The calling thread's last error, right after a function returned a null handle.
fn last_error(function: &'static str) -> NetError {
    // SAFETY: reads the calling thread's last-error value; no preconditions.
    let code = unsafe { GetLastError() }.0;
    net_error(function, code)
}

/// The Win32 code inside a `windows` error (`HRESULT_FROM_WIN32`).
fn from_error(function: &'static str, error: &windows::core::Error) -> NetError {
    let hresult = error.code().0 as u32;
    let code = if hresult & 0xFFFF_0000 == 0x8007_0000 {
        hresult & 0xFFFF
    } else {
        hresult
    };
    net_error(function, code)
}

fn refused(function: &'static str) -> NetError {
    NetError {
        function,
        code: ERROR_INVALID_PARAMETER,
        kind: NetErrorKind::Other,
    }
}

/// NUL-terminated UTF-16; text containing NUL is refused.
fn wide(text: &str, function: &'static str) -> Result<Vec<u16>, NetError> {
    if text.contains('\0') {
        return Err(refused(function));
    }
    Ok(text.encode_utf16().chain(std::iter::once(0)).collect())
}

/// `WinHttpOpen` (synchronous) and the session options: TLS 1.2 and 1.3 only, no redirects.
fn open_handle(
    user_agent: &str,
    access: WINHTTP_ACCESS_TYPE,
    proxy: Option<&str>,
) -> Result<Handle, NetError> {
    let agent = wide(user_agent, "WinHttpOpen")?;
    let proxy = proxy.map(|proxy| wide(proxy, "WinHttpOpen")).transpose()?;
    let proxy_name = proxy
        .as_ref()
        .map_or(PCWSTR::null(), |proxy| PCWSTR(proxy.as_ptr()));
    // SAFETY: `agent` and `proxy` (when present) are NUL-terminated and outlive the call; a null
    // bypass list means none; flags 0 = synchronous mode.
    let raw = unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            access,
            proxy_name,
            PCWSTR::null(),
            0,
        )
    };
    if raw.is_null() {
        return Err(last_error("WinHttpOpen"));
    }
    let handle = Handle(raw);
    set_u32(
        &handle,
        WINHTTP_OPTION_SECURE_PROTOCOLS,
        WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
        "WinHttpSetOption(WINHTTP_OPTION_SECURE_PROTOCOLS)",
    )?;
    set_u32(
        &handle,
        WINHTTP_OPTION_REDIRECT_POLICY,
        WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        "WinHttpSetOption(WINHTTP_OPTION_REDIRECT_POLICY)",
    )?;
    Ok(handle)
}

/// A DWORD option.
fn set_u32(
    handle: &Handle,
    option: u32,
    value: u32,
    function: &'static str,
) -> Result<(), NetError> {
    let bytes = value.to_ne_bytes();
    // SAFETY: the handle is open; the buffer is the 4 bytes of a DWORD, which every option set
    // here takes.
    unsafe { WinHttpSetOption(Some(handle.0.cast_const()), option, Some(&bytes)) }
        .map_err(|error| from_error(function, &error))
}

fn query_status(request: &Handle) -> Result<u16, NetError> {
    let mut status = 0u32;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: the request handle has a response; the buffer is a u32 of `len` bytes; the header is
    // chosen by index (null name) and no index is kept (null).
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
    }
    .map_err(|error| from_error("WinHttpQueryHeaders(WINHTTP_QUERY_STATUS_CODE)", &error))?;
    u16::try_from(status).map_err(|_| refused("WinHttpQueryHeaders(WINHTTP_QUERY_STATUS_CODE)"))
}

/// All response header lines, CRLF-separated, the status line first.
fn query_raw_headers(request: &Handle) -> Result<String, NetError> {
    const FUNCTION: &str = "WinHttpQueryHeaders(WINHTTP_QUERY_RAW_HEADERS_CRLF)";
    let mut len = 0u32;
    // SAFETY: the request handle has a response; a missing buffer asks for the needed length in
    // bytes, which the call stores in `len` while failing with ERROR_INSUFFICIENT_BUFFER.
    let sized = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            PCWSTR::null(),
            None,
            &mut len,
            std::ptr::null_mut(),
        )
    };
    match sized {
        Ok(()) => return Ok(String::new()),
        Err(error) if from_error(FUNCTION, &error).code == ERROR_INSUFFICIENT_BUFFER.0 => {}
        Err(error) => return Err(from_error(FUNCTION, &error)),
    }
    let mut buffer = vec![0u16; (len as usize).div_ceil(2)];
    let mut len = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` is writable for `len` bytes; the rest as above.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            PCWSTR::null(),
            Some(buffer.as_mut_ptr().cast()),
            &mut len,
            std::ptr::null_mut(),
        )
    }
    .map_err(|error| from_error(FUNCTION, &error))?;
    let units = (len as usize / 2).min(buffer.len());
    Ok(String::from_utf16_lossy(&buffer[..units]))
}

/// `Name: value` lines after the status line; lines without a colon are skipped.
fn parse_raw_headers(raw: &str) -> Vec<(String, String)> {
    raw.split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_headers() {
        let headers = parse_raw_headers(
            "HTTP/1.1 302 Found\r\nLocation: https://github.com/x\r\nSet-Cookie: a=1\r\nset-cookie: b=2\r\n\r\n",
        );
        assert_eq!(headers.len(), 3);
        let response = HttpResponse {
            request: Handle(std::ptr::null_mut()),
            _connection: Handle(std::ptr::null_mut()),
            status: 302,
            headers,
            session: PhantomData,
        };
        assert_eq!(
            response.header("location").as_deref(),
            Some("https://github.com/x")
        );
        assert_eq!(response.header("SET-COOKIE").as_deref(), Some("a=1, b=2"));
        assert_eq!(response.header("Content-Length"), None);
        assert_eq!(response.status(), 302);
    }

    #[test]
    fn error_kinds() {
        assert_eq!(kind_of(12002), NetErrorKind::Timeout);
        assert_eq!(kind_of(12007), NetErrorKind::NameNotResolved);
        assert_eq!(kind_of(12029), NetErrorKind::CannotConnect);
        assert_eq!(kind_of(12017), NetErrorKind::Cancelled);
        assert_eq!(kind_of(12175), NetErrorKind::Tls);
        assert_eq!(kind_of(12045), NetErrorKind::Tls);
        assert_eq!(kind_of(12030), NetErrorKind::Other);
        assert_eq!(kind_of(5), NetErrorKind::Other);
        let error =
            windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_2EE2_u32 as i32));
        assert_eq!(
            from_error("f", &error),
            NetError {
                function: "f",
                code: 12002,
                kind: NetErrorKind::Timeout
            }
        );
    }

    #[test]
    fn plain_http_is_refused_outside_development_builds() {
        let session = HttpSession::open("MKLM/test").unwrap();
        let request = HttpGet {
            secure: false,
            host: "github.com",
            port: 80,
            path_and_query: "/",
            accept: "*/*",
            timeouts: HttpTimeouts {
                resolve_ms: 1000,
                connect_ms: 1000,
                send_ms: 1000,
                receive_ms: 1000,
            },
        };
        // Refused before anything is sent (no network is used).
        let error = session.get(&request).unwrap_err();
        assert_eq!(error.code, ERROR_INVALID_PARAMETER);
        let nul = HttpGet {
            secure: true,
            host: "github.com\0x",
            ..request
        };
        assert_eq!(session.get(&nul).unwrap_err().code, ERROR_INVALID_PARAMETER);
        let relative = HttpGet {
            secure: true,
            path_and_query: "x",
            ..request
        };
        assert_eq!(
            session.get(&relative).unwrap_err().code,
            ERROR_INVALID_PARAMETER
        );
    }
}
