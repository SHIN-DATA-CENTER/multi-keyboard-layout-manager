//! The proxy authentication test of design m5b F.3 (SECURITY-13; FIX-VERIFICATION-4): a named
//! proxy on 127.0.0.1 answers every request with 407 (`NTLM`, `Negotiate`); with the product's
//! autologon level HIGH no `Proxy-Authorization` may reach it, and the LOW control case must see
//! one (else the test proves nothing and fails).
//!
//! Runs only as ci.yml's F.3 step runs it: `RUSTFLAGS=--cfg mklm_update_dev`,
//! `CARGO_TARGET_DIR=target\dev-update`, `cargo test -p mklm-win --features net --test net_proxy`.
//! Loopback only; nothing leaves the machine (`update.invalid` is never resolved: WinHTTP hands
//! the name to the proxy, which is this test).
//!
//! The proxy never sends an NTLM challenge (Type 2), so no response computed from credentials
//! (Type 3) can be produced in any case; the control case only proves that WinHTTP would start
//! the handshake (Type 1 / Negotiate token) under LOW.
//!
//! `AutologonLevel::High` is the product's configuration (`HttpSession::open`: autologon HIGH and
//! `WINHTTP_DISABLE_AUTHENTICATION`); `Low` is LOW with WinHTTP's automatic authentication on.
//! Measured on the development machine (2026-09-29): HIGH alone also sends no
//! `Proxy-Authorization` to this proxy; `WINHTTP_DISABLE_AUTHENTICATION` alone suppresses it even
//! under LOW.

#![cfg(all(windows, feature = "net", debug_assertions, mklm_update_dev))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mklm_win::net::{AutologonLevel, HttpGet, HttpSession, HttpTimeouts, NetErrorKind};

/// How long the proxy listens for requests after the first one (design m5b F.3: 3 seconds).
const WATCH: Duration = Duration::from_secs(3);

/// A proxy that answers every request with 407 and records every request head.
struct Proxy {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    fn start() -> Proxy {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback listener");
        let port = listener.local_addr().unwrap().port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let log = heads.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = log.clone();
                thread::spawn(move || serve(stream, &log));
            }
        });
        Proxy { port, heads }
    }

    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

/// Keeps the connection: one 407 per request head, until the client closes it.
fn serve(mut stream: TcpStream, log: &Mutex<Vec<String>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    loop {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return,
            }
            if head.len() > 64 * 1024 {
                return;
            }
        }
        log.lock()
            .unwrap()
            .push(String::from_utf8_lossy(&head).to_string());
        let answer = "HTTP/1.1 407 Proxy Authentication Required\r\n\
                      Proxy-Authenticate: NTLM\r\n\
                      Proxy-Authenticate: Negotiate\r\n\
                      Content-Length: 0\r\n\
                      Proxy-Connection: Keep-Alive\r\n\
                      Connection: Keep-Alive\r\n\r\n";
        if stream.write_all(answer.as_bytes()).is_err() {
            return;
        }
    }
}

/// The result of one GET through the proxy, and every request head the proxy saw within
/// [`WATCH`].
fn through_proxy(secure: bool, autologon: AutologonLevel) -> (String, Vec<String>) {
    let proxy = Proxy::start();
    let session =
        HttpSession::open_named_proxy("MKLM/test", &format!("127.0.0.1:{}", proxy.port), autologon)
            .expect("a named proxy session");
    let started = Instant::now();
    let request = HttpGet {
        secure,
        host: "update.invalid",
        port: if secure { 443 } else { 80 },
        path_and_query: "/latest.json",
        accept: "*/*",
        timeouts: HttpTimeouts {
            resolve_ms: 5_000,
            connect_ms: 5_000,
            send_ms: 5_000,
            receive_ms: 5_000,
        },
    };
    let outcome = match session.get(&request) {
        Ok(response) => format!("status {}", response.status()),
        Err(error) => format!("{error} ({:?})", error.kind),
    };
    if let Some(left) = WATCH.checked_sub(started.elapsed()) {
        thread::sleep(left);
    }
    drop(session);
    (outcome, proxy.heads())
}

fn has_proxy_authorization(head: &str) -> bool {
    head.to_ascii_lowercase()
        .contains("\r\nproxy-authorization:")
}

/// HIGH (what `HttpSession::open` uses): the 407 comes back, no credentials go out.
fn assert_high(secure: bool) {
    let (outcome, heads) = through_proxy(secure, AutologonLevel::High);
    assert!(!heads.is_empty(), "the proxy saw no request ({outcome})");
    assert!(
        outcome == "status 407" || outcome.contains(&format!("{:?}", NetErrorKind::ProxyAuth)),
        "{outcome}"
    );
    for head in &heads {
        assert!(!has_proxy_authorization(head), "{head}");
    }
}

/// LOW (the control case): WinHTTP starts the NTLM / Negotiate handshake with the user's
/// credentials. If it does not, the HIGH test above shows nothing on this machine.
fn assert_low_control(secure: bool) {
    let (outcome, heads) = through_proxy(secure, AutologonLevel::Low);
    assert!(
        heads.iter().any(|head| has_proxy_authorization(head)),
        "control case: WinHTTP sent no Proxy-Authorization under autologon LOW, so the HIGH test \
         proves nothing here ({outcome}); heads: {heads:#?}"
    );
}

#[test]
fn https_through_the_proxy_sends_no_credentials() {
    assert_high(true);
}

#[test]
fn https_control_case_sends_credentials_under_low() {
    assert_low_control(true);
}

#[test]
fn plain_http_through_the_proxy_sends_no_credentials() {
    assert_high(false);
}

#[test]
fn plain_http_control_case_sends_credentials_under_low() {
    assert_low_control(false);
}

#[test]
fn named_proxies_are_loopback_only() {
    for proxy in [
        "127.0.0.2:8080",
        "localhost:8080",
        "127.0.0.1",
        "127.0.0.1:",
        "127.0.0.1:0",
        "127.0.0.1:080",
        "127.0.0.1:65536",
        "http=127.0.0.1:8080",
    ] {
        assert!(
            HttpSession::open_named_proxy("MKLM/test", proxy, AutologonLevel::High).is_err(),
            "{proxy}"
        );
    }
}
