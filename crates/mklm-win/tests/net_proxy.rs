//! The proxy authentication test of design m5b F.3 (SECURITY-13; FIX-VERIFICATION-4): a named
//! proxy on 127.0.0.1 answers every `CONNECT` with 407 (`NTLM`, `Negotiate`); with the product's
//! autologon level HIGH no `Proxy-Authorization` may reach it, and the LOW control case must see
//! one (else the test proves nothing and fails).
//!
//! Runs only as ci.yml's F.3 step runs it: `RUSTFLAGS=--cfg mklm_update_dev`,
//! `CARGO_TARGET_DIR=target\dev-update`, `cargo test -p mklm-win --features net --test net_proxy`.
//! Loopback only; nothing leaves the machine (`update.invalid` is never resolved).
//!
//! Skeleton (M5b): WP-U writes the tests.

#![cfg(all(windows, feature = "net", debug_assertions, mklm_update_dev))]

use mklm_win::net::{AutologonLevel, HttpSession};

/// Until WP-U's tests land: the development-only API compiles in this configuration. Opening a
/// session sends nothing.
#[test]
fn named_proxy_sessions_exist_in_development_builds() {
    let _session = HttpSession::open_named_proxy("MKLM/test", "127.0.0.1:9", AutologonLevel::High);
}
