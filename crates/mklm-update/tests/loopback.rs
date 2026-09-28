//! The loopback HTTP tests of design m5b F.3: the fetch policy over the real WinHTTP transport
//! (`WinHttpTransport::new_without_proxy`) against a scripted server on `127.0.0.1:0` (never on all
//! interfaces, so no firewall prompt; nothing leaves the machine).
//!
//! Runs only as ci.yml's F.3 step runs it: `RUSTFLAGS=--cfg mklm_update_dev`,
//! `CARGO_TARGET_DIR=target\dev-update`, `cargo test -p mklm-update --features winhttp`.

#![cfg(all(windows, feature = "winhttp", debug_assertions, mklm_update_dev))]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mklm_update::fetch::{
    FetchError, Limits, Timeouts, TransportError, download_asset, fetch_alt_signature,
    fetch_latest_file, fetch_manifest,
};
use mklm_update::url::Endpoints;
use mklm_update::winhttp::WinHttpTransport;
use mklm_update::{Arch, SelectedAsset, Sha256Digest, Version};

/// What the server answers on one path.
#[derive(Debug, Clone)]
enum Reply {
    /// Status, extra headers, body with `Content-Length`.
    Full(u16, Vec<(String, String)>, Vec<u8>),
    /// Status, extra headers (may include a wrong `Content-Length`), raw body bytes as given.
    Raw(u16, Vec<(String, String)>, Vec<u8>),
    /// 200 with `Transfer-Encoding: chunked` in these chunks.
    Chunked(Vec<Vec<u8>>),
    /// Reads the request and never answers.
    Silent,
    /// Waits, then answers.
    Delayed(Duration, Box<Reply>),
}

fn full(status: u16, body: &[u8]) -> Reply {
    Reply::Full(status, Vec::new(), body.to_vec())
}

fn redirect(status: u16, location: &str) -> Reply {
    Reply::Full(
        status,
        vec![("Location".to_string(), location.to_string())],
        Vec::new(),
    )
}

/// A scripted HTTP/1.1 server on 127.0.0.1; counts connections and keeps every request head.
struct Server {
    port: u16,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(routes: Vec<(&str, Reply)>) -> Server {
        Self::start_on("127.0.0.1", routes).expect("a loopback listener")
    }

    fn start_on(address: &str, routes: Vec<(&str, Reply)>) -> Option<Server> {
        let listener = TcpListener::bind((address, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let routes: Arc<HashMap<String, Reply>> = Arc::new(
            routes
                .into_iter()
                .map(|(path, reply)| (path.to_string(), reply))
                .collect(),
        );
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (count, log) = (connections.clone(), requests.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                count.fetch_add(1, Ordering::SeqCst);
                let (routes, log) = (routes.clone(), log.clone());
                thread::spawn(move || handle(stream, &routes, &log));
            }
        });
        Some(Server {
            port,
            connections,
            requests,
        })
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints::loopback(&self.base()).unwrap()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn handle(mut stream: TcpStream, routes: &HashMap<String, Reply>, log: &Mutex<Vec<String>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
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
    let head = String::from_utf8_lossy(&head).to_string();
    log.lock().unwrap().push(head.clone());
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let reply = routes.get(&path).cloned().unwrap_or_else(|| full(404, b""));
    respond(&mut stream, reply);
}

fn respond(stream: &mut TcpStream, reply: Reply) {
    let reason = |status: u16| match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        404 => "Not Found",
        _ => "Status",
    };
    let write_head = |stream: &mut TcpStream, status: u16, headers: &[(String, String)]| {
        let mut text = format!(
            "HTTP/1.1 {status} {}\r\nConnection: close\r\n",
            reason(status)
        );
        for (name, value) in headers {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        text.push_str("\r\n");
        let _ = stream.write_all(text.as_bytes());
    };
    match reply {
        Reply::Full(status, mut headers, body) => {
            headers.push(("Content-Length".to_string(), body.len().to_string()));
            write_head(stream, status, &headers);
            let _ = stream.write_all(&body);
        }
        Reply::Raw(status, headers, body) => {
            write_head(stream, status, &headers);
            let _ = stream.write_all(&body);
        }
        Reply::Chunked(chunks) => {
            write_head(
                stream,
                200,
                &[("Transfer-Encoding".to_string(), "chunked".to_string())],
            );
            for chunk in chunks {
                let _ = stream.write_all(format!("{:x}\r\n", chunk.len()).as_bytes());
                let _ = stream.write_all(&chunk);
                let _ = stream.write_all(b"\r\n");
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        Reply::Silent => {
            thread::sleep(Duration::from_secs(10));
            return;
        }
        Reply::Delayed(delay, reply) => {
            thread::sleep(delay);
            respond(stream, *reply);
            return;
        }
    }
    let _ = stream.flush();
    // Let the client read everything before the connection closes.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    thread::sleep(Duration::from_millis(200));
}

fn transport() -> WinHttpTransport {
    WinHttpTransport::new_without_proxy("MKLM/0.2.0 (Windows; x64; +test)").unwrap()
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// Short limits so that a failing test does not hang.
fn limits() -> Limits {
    Limits {
        timeouts: Timeouts {
            resolve: Duration::from_secs(5),
            connect: Duration::from_secs(5),
            send: Duration::from_secs(5),
            receive: Duration::from_secs(5),
        },
        total: Duration::from_secs(20),
        max_redirects: 5,
    }
}

fn latest(
    server: &Server,
    name: &str,
    max_len: u64,
) -> Result<(Vec<u8>, Option<String>), FetchError> {
    fetch_latest_file(
        &mut transport(),
        &server.endpoints(),
        name,
        max_len,
        &limits(),
        &no_cancel(),
    )
}

#[test]
fn a_small_body() {
    let server = Server::start(vec![(
        "/releases/latest/download/hello.txt",
        full(200, b"hello"),
    )]);
    let (body, tag) = latest(&server, "hello.txt", 1024).unwrap();
    assert_eq!(body, b"hello");
    assert_eq!(tag, None);
    // Only User-Agent and Accept of our own; no cookies, credentials or compression.
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let head = requests[0].to_ascii_lowercase();
    assert!(
        head.starts_with("get /releases/latest/download/hello.txt http/1.1\r\n"),
        "{head}"
    );
    assert!(
        head.contains("user-agent: mklm/0.2.0 (windows; x64; +test)\r\n"),
        "{head}"
    );
    assert!(head.contains("accept: */*\r\n"), "{head}");
    for absent in ["accept-encoding", "cookie", "authorization"] {
        assert!(!head.contains(absent), "{absent}: {head}");
    }
}

#[test]
fn the_manifest_through_two_redirects_and_its_tag() {
    let cdn = Server::start(vec![
        ("/asset/manifest?sig=1", full(200, b"{\"schema\":1}")),
        ("/asset/signature", full(200, b"signature")),
    ]);
    let github = Server::start(vec![
        (
            "/releases/latest/download/latest.json",
            redirect(302, "/releases/download/v0.2.1/latest.json"),
        ),
        (
            "/releases/download/v0.2.1/latest.json",
            redirect(302, &format!("{}/asset/manifest?sig=1", cdn.base())),
        ),
        (
            "/releases/download/v0.2.1/latest.json.minisig",
            redirect(302, &format!("{}/asset/signature", cdn.base())),
        ),
    ]);
    let fetched = fetch_manifest(
        &mut transport(),
        &github.endpoints(),
        &limits(),
        &no_cancel(),
    )
    .unwrap();
    assert_eq!(fetched.manifest, b"{\"schema\":1}");
    assert_eq!(fetched.signature, b"signature");
    assert_eq!(fetched.tag.as_deref(), Some("v0.2.1"));
}

#[test]
fn sha256sums_through_latest_gives_its_tag() {
    let server = Server::start(vec![
        (
            "/releases/latest/download/SHA256SUMS",
            redirect(302, "/releases/download/v0.1.0/SHA256SUMS"),
        ),
        ("/releases/download/v0.1.0/SHA256SUMS", full(200, b"sums")),
    ]);
    let (body, tag) = latest(&server, "SHA256SUMS", 1024).unwrap();
    assert_eq!(body, b"sums");
    assert_eq!(tag.as_deref(), Some("v0.1.0"));
}

#[test]
fn a_missing_alternate_signature() {
    let server = Server::start(vec![]);
    let result = fetch_alt_signature(
        &mut transport(),
        &server.endpoints(),
        Some("v0.2.1"),
        &limits(),
        &no_cancel(),
    );
    assert_eq!(result, Ok(None));
    assert!(
        server.requests()[0]
            .starts_with("GET /releases/download/v0.2.1/latest.json.alt.minisig HTTP/1.1\r\n")
    );
}

#[test]
fn six_redirects_are_too_many() {
    let mut routes = vec![("/releases/latest/download/x", redirect(301, "/hop/1"))];
    let paths: Vec<String> = (1..=6).map(|n| format!("/hop/{n}")).collect();
    for (n, path) in paths.iter().enumerate() {
        routes.push((path.as_str(), redirect(307, &format!("/hop/{}", n + 2))));
    }
    let server = Server::start(routes);
    assert_eq!(
        latest(&server, "x", 1024),
        Err(FetchError::TooManyRedirects)
    );
    assert_eq!(server.requests().len(), 6);
}

#[test]
fn redirects_elsewhere_never_connect() {
    let target = Server::start(vec![("/x", full(200, b"x"))]);
    let other_loopback = Server::start_on("127.0.0.2", vec![("/x", full(200, b"x"))]);
    let mut locations = vec![
        format!("https://127.0.0.1:{}/x", target.port),
        format!("http://localhost:{}/x", target.port),
        "http://127.0.0.1/x".to_string(),
    ];
    if let Some(other) = &other_loopback {
        locations.push(format!("http://127.0.0.2:{}/x", other.port));
    }
    for location in &locations {
        let server = Server::start(vec![(
            "/releases/latest/download/x",
            redirect(302, location),
        )]);
        assert_eq!(
            latest(&server, "x", 1024),
            Err(FetchError::RedirectNotAllowed {
                location: location.clone()
            }),
            "{location}"
        );
    }
    assert_eq!(target.connections.load(Ordering::SeqCst), 0);
    if let Some(other) = &other_loopback {
        assert_eq!(other.connections.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn size_limits_on_the_wire() {
    // Content-Length over the limit: refused unread.
    let server = Server::start(vec![(
        "/releases/latest/download/big",
        Reply::Raw(
            200,
            vec![("Content-Length".to_string(), "70000".to_string())],
            b"{}".to_vec(),
        ),
    )]);
    assert_eq!(
        latest(&server, "big", 65_536),
        Err(FetchError::TooLarge { limit: 65_536 })
    );
    // Chunked, one byte over.
    let server = Server::start(vec![(
        "/releases/latest/download/chunked",
        Reply::Chunked(vec![vec![b'a'; 40_000], vec![b'b'; 25_537]]),
    )]);
    assert_eq!(
        latest(&server, "chunked", 65_536),
        Err(FetchError::TooLarge { limit: 65_536 })
    );
    // Chunked, exactly the limit.
    let server = Server::start(vec![(
        "/releases/latest/download/chunked",
        Reply::Chunked(vec![vec![b'a'; 40_000], vec![b'b'; 25_536]]),
    )]);
    assert_eq!(latest(&server, "chunked", 65_536).unwrap().0.len(), 65_536);
}

fn installer_asset(content: &[u8]) -> SelectedAsset {
    SelectedAsset {
        arch: Arch::X64,
        name: "MKLM-Setup-0.2.1-x64.exe".to_string(),
        size: content.len() as u64,
        sha256: Sha256Digest::of(content),
    }
}

fn download(reply: Reply, asset: &SelectedAsset) -> (Result<(), FetchError>, Vec<u8>) {
    let server = Server::start(vec![(
        "/releases/download/v0.2.1/MKLM-Setup-0.2.1-x64.exe",
        reply,
    )]);
    let mut sink = Vec::new();
    let result = download_asset(
        &mut transport(),
        &server.endpoints(),
        &Version::new(0, 2, 1),
        asset,
        &limits(),
        &mut sink,
        &mut |_| {},
        &no_cancel(),
    );
    if let Some(head) = server.requests().first() {
        assert!(
            head.to_ascii_lowercase()
                .contains("accept: application/octet-stream\r\n"),
            "{head}"
        );
    }
    (result, sink)
}

#[test]
fn installer_downloads() {
    let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let asset = installer_asset(&content);
    let (result, sink) = download(full(200, &content), &asset);
    assert_eq!(result, Ok(()));
    assert_eq!(sink, content);
    // Short, long (streamed without Content-Length), other content.
    let short = content[..content.len() - 1].to_vec();
    assert_eq!(
        download(Reply::Chunked(vec![short.clone()]), &asset).0,
        Err(FetchError::SizeMismatch {
            expected: 300_000,
            received: 299_999
        })
    );
    let mut long = content.clone();
    long.push(1);
    assert_eq!(
        download(Reply::Chunked(vec![long.clone()]), &asset).0,
        Err(FetchError::SizeMismatch {
            expected: 300_000,
            received: 300_001
        })
    );
    assert_eq!(
        download(full(200, &short), &asset).0,
        Err(FetchError::SizeMismatch {
            expected: 300_000,
            received: 299_999
        })
    );
    let mut other = content.clone();
    other[100] ^= 1;
    assert_eq!(
        download(full(200, &other), &asset).0,
        Err(FetchError::HashMismatch)
    );
}

#[test]
fn a_silent_server_times_out() {
    let server = Server::start(vec![("/releases/latest/download/x", Reply::Silent)]);
    let limits = Limits {
        timeouts: Timeouts {
            receive: Duration::from_secs(1),
            ..limits().timeouts
        },
        ..limits()
    };
    let started = Instant::now();
    let result = fetch_latest_file(
        &mut transport(),
        &server.endpoints(),
        "x",
        1024,
        &limits,
        &no_cancel(),
    );
    assert_eq!(result, Err(FetchError::Transport(TransportError::Timeout)));
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn the_whole_fetch_has_a_deadline() {
    let delay = Duration::from_millis(400);
    let mut routes = vec![(
        "/releases/latest/download/x",
        Reply::Delayed(delay, Box::new(redirect(302, "/hop/1"))),
    )];
    let paths: Vec<String> = (1..=5).map(|n| format!("/hop/{n}")).collect();
    for (n, path) in paths.iter().enumerate() {
        routes.push((
            path.as_str(),
            Reply::Delayed(delay, Box::new(redirect(302, &format!("/hop/{}", n + 2)))),
        ));
    }
    let server = Server::start(routes);
    let limits = Limits {
        total: Duration::from_secs(1),
        ..limits()
    };
    let result = fetch_latest_file(
        &mut transport(),
        &server.endpoints(),
        "x",
        1024,
        &limits,
        &no_cancel(),
    );
    assert_eq!(result, Err(FetchError::DeadlineExceeded));
}

#[test]
fn status_codes_on_the_wire() {
    for (status, expected) in [
        (404, FetchError::NotFound),
        (429, FetchError::RateLimited { status: 429 }),
        (500, FetchError::HttpStatus { status: 500 }),
    ] {
        let server = Server::start(vec![("/releases/latest/download/x", full(status, b"no"))]);
        assert_eq!(latest(&server, "x", 1024), Err(expected), "{status}");
    }
}

/// SECURITY-13: a server asking for NTLM or Negotiate gets no credentials. Autologon HIGH alone
/// does not cover a server named by address (Microsoft Learn): with HIGH alone WinHTTP sent an NTLM
/// negotiate message here; `WINHTTP_DISABLE_AUTHENTICATION` (mklm-win `net`) stops it.
#[test]
fn server_authentication_is_never_answered() {
    let server = Server::start(vec![(
        "/releases/latest/download/x",
        Reply::Full(
            401,
            vec![
                ("WWW-Authenticate".to_string(), "NTLM".to_string()),
                ("WWW-Authenticate".to_string(), "Negotiate".to_string()),
            ],
            b"denied".to_vec(),
        ),
    )]);
    assert_eq!(
        latest(&server, "x", 1024),
        Err(FetchError::HttpStatus { status: 401 })
    );
    // Give a (wrong) retry the time to arrive.
    thread::sleep(Duration::from_millis(500));
    let requests = server.requests();
    assert!(!requests.is_empty());
    for head in &requests {
        assert!(
            !head.to_ascii_lowercase().contains("\r\nauthorization:"),
            "{head}"
        );
    }
}

/// Without a proxy session only the mapping of the status is tested: WinHTTP does not treat this
/// server as a proxy (the proxy path is `mklm-win`'s `tests/net_proxy.rs`).
#[test]
fn a_407_is_proxy_authentication() {
    let server = Server::start(vec![(
        "/releases/latest/download/x",
        Reply::Full(
            407,
            vec![("Proxy-Authenticate".to_string(), "NTLM".to_string())],
            Vec::new(),
        ),
    )]);
    assert_eq!(
        latest(&server, "x", 1024),
        Err(FetchError::Transport(TransportError::ProxyAuthRequired))
    );
}

#[test]
fn compressed_answers_are_refused() {
    let server = Server::start(vec![(
        "/releases/latest/download/x",
        Reply::Full(
            200,
            vec![("Content-Encoding".to_string(), "gzip".to_string())],
            b"\x1f\x8b".to_vec(),
        ),
    )]);
    assert_eq!(
        latest(&server, "x", 1024),
        Err(FetchError::UnexpectedEncoding)
    );
}

#[test]
fn the_cancel_flag_stops_a_download() {
    let content = vec![5u8; 2 * 1024 * 1024];
    let asset = installer_asset(&content);
    let server = Server::start(vec![(
        "/releases/download/v0.2.1/MKLM-Setup-0.2.1-x64.exe",
        full(200, &content),
    )]);
    let cancel = AtomicBool::new(false);
    let mut sink = Vec::new();
    let result = download_asset(
        &mut transport(),
        &server.endpoints(),
        &Version::new(0, 2, 1),
        &asset,
        &limits(),
        &mut sink,
        &mut |_| cancel.store(true, Ordering::SeqCst),
        &cancel,
    );
    assert_eq!(result, Err(FetchError::Cancelled));
    assert!(sink.len() < content.len());
}
