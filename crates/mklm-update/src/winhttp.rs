//! [`Transport`] over WinHTTP (`mklm_win::net`, feature `winhttp`, Windows; design m5b A.7, A.9).
//!
//! WP-U implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use mklm_win::net::HttpSession;

use crate::fetch::{Response, Timeouts, Transport, TransportError};
use crate::url::Url;

#[derive(Debug)]
pub struct WinHttpTransport {
    session: HttpSession,
}

impl WinHttpTransport {
    /// Automatic proxy, autologon HIGH, no credentials (design m5b A.7).
    pub fn new(user_agent: &str) -> Result<WinHttpTransport, TransportError> {
        Err(skeleton()) // Skeleton (M5b): WP-U
    }

    /// No proxy, for the loopback tests and the rehearsal.
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub fn new_without_proxy(user_agent: &str) -> Result<WinHttpTransport, TransportError> {
        Err(skeleton()) // Skeleton (M5b): WP-U
    }
}

impl Transport for WinHttpTransport {
    fn get(
        &mut self,
        url: &Url,
        accept: &str,
        timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError> {
        Err(skeleton()) // Skeleton (M5b): WP-U
    }
}

/// What the WP-0 skeleton returns (design m5b G.2): `ERROR_NOT_SUPPORTED`.
fn skeleton() -> TransportError {
    TransportError::Other { code: 50 }
}
