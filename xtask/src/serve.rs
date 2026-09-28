//! `serve-releases --dir <dist-dev root> [--port N]` (design m5b B.3, F.6): the rehearsal's
//! stand-in for GitHub's release downloads, on 127.0.0.1 only.
//!
//! `<root>\<X.Y.Z>\` is the release `vX.Y.Z`. `/releases/latest/download/<name>` answers 302 to
//! `/releases/download/v<latest>/<name>` (the same shape as GitHub's first redirect, so the
//! client finds the tag); `/releases/download/v<X.Y.Z>/<name>` serves the file. The latest release
//! is the newest folder that holds both `latest.json` and `latest.json.minisig`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use mklm_update::Version;
use mklm_update::version::parse_release_version;

/// The port of design m5b F.6.
pub const DEFAULT_PORT: u16 = 8421;

/// Serves until the process ends.
pub fn serve(root: &Path, port: u16, out: &mut dyn Write) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("listening on 127.0.0.1:{port}"))?;
    writeln!(
        out,
        "Serving {} on http://127.0.0.1:{port} (latest: {}). Ctrl+C stops.",
        root.display(),
        latest_version(root).map_or("none yet".to_string(), |version| format!("v{version}"))
    )?;
    out.flush()?;
    serve_listener(
        listener,
        root.to_path_buf(),
        Arc::new(AtomicBool::new(false)),
    );
    Ok(())
}

/// The accept loop; returns once `stop` is set (checked between connections).
pub fn serve_listener(listener: TcpListener, root: PathBuf, stop: Arc<AtomicBool>) {
    let port = listener
        .local_addr()
        .map(|address| address.port())
        .unwrap_or(0);
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let root = root.clone();
        std::thread::spawn(move || handle(stream, &root, port));
    }
}

/// The newest `X.Y.Z` folder with a manifest and its signature.
pub fn latest_version(root: &Path) -> Option<Version> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let version = parse_release_version(&entry.file_name().to_string_lossy()).ok()?;
            let dir = entry.path();
            (dir.join(mklm_update::MANIFEST_NAME).is_file()
                && dir.join(mklm_update::SIGNATURE_NAME).is_file())
            .then_some(version)
        })
        .max_by(|a, b| a.cmp_precedence(b))
}

/// A file name that stays inside its folder.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Status, extra headers, body.
pub fn answer(root: &Path, port: u16, path: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let not_found = (404, Vec::new(), b"not found".to_vec());
    if let Some(name) = path.strip_prefix("/releases/latest/download/") {
        if !plain_name(name) {
            return not_found;
        }
        return match latest_version(root) {
            Some(latest) => (
                302,
                vec![(
                    "Location".to_string(),
                    format!("http://127.0.0.1:{port}/releases/download/v{latest}/{name}"),
                )],
                Vec::new(),
            ),
            None => not_found,
        };
    }
    if let Some(rest) = path.strip_prefix("/releases/download/v") {
        let Some((version, name)) = rest.split_once('/') else {
            return not_found;
        };
        if parse_release_version(version).is_err() || !plain_name(name) {
            return not_found;
        }
        return match std::fs::read(root.join(version).join(name)) {
            Ok(body) => (
                200,
                vec![(
                    "Content-Type".to_string(),
                    "application/octet-stream".to_string(),
                )],
                body,
            ),
            Err(_) => not_found,
        };
    }
    not_found
}

fn handle(mut stream: TcpStream, root: &Path, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
        if head.len() > 16 * 1024 {
            return;
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut words = head.split_whitespace();
    let method = words.next().unwrap_or_default();
    let path = words.next().unwrap_or("/");
    let (status, headers, body) = if method == "GET" || method == "HEAD" {
        answer(root, port, path)
    } else {
        (405, Vec::new(), Vec::new())
    };
    let reason = match status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    let mut text = format!(
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    let _ = stream.write_all(text.as_bytes());
    if method != "HEAD" {
        let _ = stream.write_all(&body);
    }
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn release(root: &Path, version: &str, signed: bool) {
        let dir = root.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("latest.json"),
            format!("{{\"version\":\"{version}\"}}"),
        )
        .unwrap();
        if signed {
            std::fs::write(dir.join("latest.json.minisig"), "sig").unwrap();
        }
        std::fs::write(dir.join(format!("MKLM-Setup-{version}-x64.exe")), "exe").unwrap();
    }

    #[test]
    fn answers_like_github() {
        let root = TempDir::new("serve");
        release(root.path(), "0.2.0", true);
        release(root.path(), "0.2.1", true);
        release(root.path(), "0.10.0", false);
        std::fs::create_dir_all(root.path().join("notes")).unwrap();
        assert_eq!(latest_version(root.path()), Some(Version::new(0, 2, 1)));
        let (status, headers, _) =
            answer(root.path(), 8421, "/releases/latest/download/latest.json");
        assert_eq!(status, 302);
        assert_eq!(
            headers[0].1,
            "http://127.0.0.1:8421/releases/download/v0.2.1/latest.json"
        );
        let (status, _, body) = answer(root.path(), 8421, "/releases/download/v0.2.1/latest.json");
        assert_eq!(status, 200);
        assert_eq!(body, b"{\"version\":\"0.2.1\"}");
        for path in [
            "/releases/download/v0.2.1/missing",
            "/releases/download/v0.2.1/../0.2.0/latest.json",
            "/releases/download/v0.2.1/..",
            "/releases/download/0.2.1/latest.json",
            "/releases/download/v0.2/latest.json",
            "/releases/latest/download/..%2Fx",
            "/other",
        ] {
            assert_eq!(answer(root.path(), 8421, path).0, 404, "{path}");
        }
        let empty = TempDir::new("serve-empty");
        assert_eq!(latest_version(empty.path()), None);
        assert_eq!(
            answer(empty.path(), 1, "/releases/latest/download/latest.json").0,
            404
        );
    }

    #[test]
    fn serves_on_loopback() {
        let root = TempDir::new("serve-socket");
        release(root.path(), "0.2.1", true);
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let (path, flag) = (root.path().to_path_buf(), stop.clone());
        let server = std::thread::spawn(move || serve_listener(listener, path, flag));
        let get = |path: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            text
        };
        let redirect = get("/releases/latest/download/latest.json.minisig");
        assert!(redirect.starts_with("HTTP/1.1 302 Found\r\n"), "{redirect}");
        assert!(redirect.contains(&format!(
            "Location: http://127.0.0.1:{port}/releases/download/v0.2.1/latest.json.minisig\r\n"
        )));
        let file = get("/releases/download/v0.2.1/latest.json.minisig");
        assert!(
            file.starts_with("HTTP/1.1 200 OK\r\n") && file.ends_with("\r\n\r\nsig"),
            "{file}"
        );
        stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", port));
        server.join().unwrap();
    }
}
