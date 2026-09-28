//! Shared by the update tests of mklm-client (design m5b F.4): manifests signed with throwaway
//! keys, a release played by a fake [`Transport`], and a scratch cache.
//!
//! The keys were generated once with the `minisign` crate for these tests and their secret halves
//! thrown away; only the public keys and the signatures are here. The texts are built with
//! explicit `\n` (a checkout that turns line ends into CRLF must not change the signed bytes).
//!
//! These tests run the real `mklm_update` verification and fetch policy (WP-U). While the M5b
//! skeleton stands in for it, [`wp_u_ready`] is false and the tests return early, saying so.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use mklm_client::update::env::{Availability, UpdateEnv};
use mklm_update::fetch::{Response, Timeouts, Transport, TransportError};
use mklm_update::url::{Endpoints, Url};
use mklm_update::{Arch, KeyRole, Sha256Digest, TrustAnchors, Version};

/// Key IDs of the throwaway keys.
pub const PRIMARY_ID: &str = "DE84F116B8548221";
pub const BACKUP_ID: &str = "091AA2ADAF923FA0";
pub const UNKNOWN_ID: &str = "5EE134AEFB6D6E15";
pub const PRIMARY_PUB: &str = "RWQhglS4FvGE3p4oMhGZzsWYtFxKbGfjFFg8VIZACF+rUWiOd1SAr4hJ";
pub const BACKUP_PUB: &str = "RWSgP5KvraIaCYiLZGOG5xpVWurCrGnJdPwGbt+ROH2rHkQBa/ENuLGV";

/// `issued_at` of the new manifests (2026-10-15 00:00 UTC) and of the older one.
pub const T1: u64 = 1_792_022_400;
pub const T0: u64 = T1 - 10 * 86_400;
/// When the tests check: an hour after T1.
pub const NOW: u64 = T1 + 3_600;

pub const REPO_PATH: &str = "/SHIN-DATA-CENTER/multi-keyboard-layout-manager";

/// The installers of the fixtures: patterns of known length and SHA-256.
pub fn installer_bytes(name: &str) -> Option<Vec<u8>> {
    let pattern = |len: usize, a: usize, b: usize, m: usize| -> Vec<u8> {
        (0..len).map(|i| ((i * a + b) % m) as u8).collect()
    };
    Some(match name {
        "MKLM-Setup-0.2.1-x64.exe" => pattern(100_000, 31, 7, 251),
        "MKLM-Setup-0.2.1-arm64.exe" => pattern(80_000, 17, 3, 253),
        "MKLM-Setup-0.2.0-x64.exe" => pattern(90_000, 13, 5, 241),
        "MKLM-Setup-0.2.0-arm64.exe" => pattern(70_000, 11, 9, 239),
        _ => return None,
    })
}

const SHA_021_X64: &str = "08d042cceab8034d08c870e707f331cac9f42321044406bcccd156c7258229ab";
const SHA_021_ARM64: &str = "127eacc0b82b026dcf07b4d8fb9022783a7656ff497b11206260308ebe1616aa";
const SHA_020_X64: &str = "3a86b4fee62d992498beb340111cf74450c1074ae5b99034eb3b0b84e4fd3c9c";
const SHA_020_ARM64: &str = "10a0ca4369bd3c24df7685943d21174039b1eb7cf4991bf45c3472fd4ef6dc7c";

/// A manifest in the canonical form xtask writes (design m5b A.3).
fn manifest(version: &str, issued_at: u64, key_ids: &[&str], min_from: Option<&str>) -> String {
    let (x64, arm64, x64_size, arm64_size) = match version {
        "0.2.1" => (SHA_021_X64, SHA_021_ARM64, 100_000, 80_000),
        _ => (SHA_020_X64, SHA_020_ARM64, 90_000, 70_000),
    };
    let ids = key_ids
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let min = min_from
        .map(|min| format!("  \"min_from_version\": \"{min}\",\n"))
        .unwrap_or_default();
    format!(
        "{{\n  \"schema\": 1,\n  \"product\": \"MKLM\",\n  \"channel\": \"stable\",\n  \"version\": \"{version}\",\n  \"issued_at\": {issued_at},\n  \"expires\": {},\n  \"key_ids\": [{ids}],\n  \"revoked_keys\": [],\n{min}  \"assets\": [\n    {{\n      \"arch\": \"x64\",\n      \"name\": \"MKLM-Setup-{version}-x64.exe\",\n      \"size\": {x64_size},\n      \"sha256\": \"{x64}\"\n    }},\n    {{\n      \"arch\": \"arm64\",\n      \"name\": \"MKLM-Setup-{version}-arm64.exe\",\n      \"size\": {arm64_size},\n      \"sha256\": \"{arm64}\"\n    }}\n  ]\n}}\n",
        issued_at + 180 * 86_400
    )
}

fn signature(lines: [&str; 3]) -> String {
    format!(
        "untrusted comment: signature from a throwaway test key\n{}\ntrusted comment: {}\n{}\n",
        lines[0], lines[1], lines[2]
    )
}

/// A signed release: the manifest, its main signature and, during a key transition, the
/// alternate one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: &'static str,
    pub manifest: String,
    pub main: String,
    pub alt: Option<String>,
}

/// 0.2.1, issued at T1, signed by the primary key.
pub fn new_release() -> Release {
    Release {
        version: "0.2.1",
        manifest: manifest("0.2.1", T1, &[PRIMARY_ID], None),
        main: signature([
            "RUQhglS4FvGE3l33ao2GSlF/V0CHKqh+ytjOFurBMH+xsNTQQ59zsDtEa/KxLw81ysNJV3uizL2+6pRJcwRTcuyO+2qjKVTi/wY=",
            "mklm-latest-json v1 version=0.2.1 issued_at=1792022400",
            "4NFZR4UKACSoWKMREZKQMsIh1/JDXmibFVGKmvCShkkjErENRMivY0CEUYRV+qExN+H8PezKkO3u1yhw1wdBDg==",
        ]),
        alt: None,
    }
}

/// 0.2.0, issued at T0 (older), signed by the primary key.
pub fn old_release() -> Release {
    Release {
        version: "0.2.0",
        manifest: manifest("0.2.0", T0, &[PRIMARY_ID], None),
        main: signature([
            "RUQhglS4FvGE3q3o1eMx+oT2wAc2DOSQQTevPvcwNAT+hgGHQ6gdn+NBRdkJeAdohRN8GGuFyuhNDwusrFGr8ZqQVLDw6wv+3QM=",
            "mklm-latest-json v1 version=0.2.0 issued_at=1791158400",
            "NDI2X5PxCxp2D/MCNnPU5KT+4q1Jtvqy7usQsOEueCv17PF7nBmoN2QbaZ7WGsTAqgqehAfffBRLsop+fpixBA==",
        ]),
        alt: None,
    }
}

/// 0.2.1, issued at T1 + 60: the main signature by a key this build does not know, the alternate
/// by the backup key (a key transition, design m5b B.7).
pub fn transition_release() -> Release {
    Release {
        version: "0.2.1",
        manifest: manifest("0.2.1", T1 + 60, &[UNKNOWN_ID, BACKUP_ID], None),
        main: signature([
            "RUQVbm37rjThXuvnfbZDgGXhMjHQ4fZoKSVfzg/zmVcwpOcPmmoN/jo3RMzSi7kZMelQPzoAUI8CBBeh0XDAJl8zxoOTDyzKaA4=",
            "mklm-latest-json v1 version=0.2.1 issued_at=1792022460",
            "V2Fxpc7XnF3fBkXy5vDEPCNvpllczykrisex/acbIUNfoks9FKR8+RyIl4y1SEuCHUtZ0Su30Mr7SfvD95bYCg==",
        ]),
        alt: Some(signature([
            "RUSgP5KvraIaCZEahJpQlmnD+eo/hjZsqI2wCoJo/+okEpjsJa5PRWD7+UjNEj7HUu6Ax78zWVe6WD+vvM7mwFWUIJl7Qyz88gs=",
            "mklm-latest-json v1 version=0.2.1 issued_at=1792022460",
            "2uFiNxg5b9WTtheBQ5Qe/TwOhbBvKVehWSjfbtedmosRqVENNyiyNlftaeu3C2M/I/6JrvHtj94wrPigjA6bAA==",
        ])),
    }
}

/// 0.2.1 that needs at least 0.2.0 (`min_from_version`).
pub fn manual_release() -> Release {
    Release {
        version: "0.2.1",
        manifest: manifest("0.2.1", T1, &[PRIMARY_ID], Some("0.2.0")),
        main: signature([
            "RUQhglS4FvGE3nzJ8988LQd1iYyr/UERdaoeiE1dSClKPlBONhKbjHWJG0QHiJTqLwvvQ0A1gs9WPBSPvvOYALfalA2V+kQsyAI=",
            "mklm-latest-json v1 version=0.2.1 issued_at=1792022400",
            "eYw1N3L+lThm1XcCjcdGGijRYW8lhJNn9n6rXgbAPBcBELpVXTIplcfSxz75DD4uqVxw3LieAIf9tY7ku++5AA==",
        ]),
        alt: None,
    }
}

/// The anchors of the tests: the primary and the backup key.
pub fn anchors() -> TrustAnchors {
    TrustAnchors::from_keys(
        &[
            (KeyRole::Primary, PRIMARY_PUB),
            (KeyRole::Backup, BACKUP_PUB),
        ],
        &[],
    )
    .expect("the test keys")
}

/// The real verification and fetch policy are there (not the m5b skeleton's stubs).
pub fn wp_u_ready() -> bool {
    let ready = TrustAnchors::from_keys(&[(KeyRole::Primary, PRIMARY_PUB)], &[]).is_ok()
        && Sha256Digest::of(b"abc") != Sha256Digest([0; 32])
        && Url::parse(
            "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager",
            Endpoints::production().policy(),
        )
        .is_ok();
    if !ready {
        eprintln!("skipped: mklm-update is the m5b skeleton (runs after the WP-U merge)");
    }
    ready
}

/// This build installed as 0.2.0 (or `installed`), updates available.
pub fn env(installed: &str, cache_dir: PathBuf) -> UpdateEnv {
    UpdateEnv {
        installed: Version::parse(installed).unwrap(),
        arch: Arch::of_this_build(),
        native: None,
        availability: Availability::Available,
        anchors: Some(anchors()),
        endpoints: Endpoints::production(),
        install_dir: PathBuf::from(r"C:\Program Files\SHIN DATA CENTER\MKLM"),
        cache_dir,
    }
}

/// GitHub as the fetch policy sees it: `releases/latest/download/<name>` redirects to
/// `releases/download/v<version>/<name>`, which serves the file (design m5b A.5, A.6).
#[derive(Debug, Clone)]
pub struct FakeGitHub {
    pub release: Release,
    /// The paths requested, in order.
    pub requests: Vec<String>,
}

impl FakeGitHub {
    pub fn new(release: Release) -> FakeGitHub {
        FakeGitHub {
            release,
            requests: Vec::new(),
        }
    }

    fn file(&self, name: &str) -> Option<Vec<u8>> {
        match name {
            "latest.json" => Some(self.release.manifest.clone().into_bytes()),
            "latest.json.minisig" => Some(self.release.main.clone().into_bytes()),
            "latest.json.alt.minisig" => self.release.alt.clone().map(String::into_bytes),
            other => installer_bytes(other),
        }
    }
}

#[derive(Debug)]
struct FakeResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    at: usize,
}

impl Response for FakeResponse {
    fn status(&self) -> u16 {
        self.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers.get(&name.to_ascii_lowercase()).cloned()
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let len = buf.len().min(self.body.len() - self.at).min(16 * 1024);
        buf[..len].copy_from_slice(&self.body[self.at..self.at + len]);
        self.at += len;
        Ok(len)
    }
}

impl Transport for FakeGitHub {
    fn get(
        &mut self,
        url: &Url,
        _accept: &str,
        _timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError> {
        let path = url.path_and_query().to_string();
        self.requests.push(path.clone());
        let mut headers = BTreeMap::new();
        let latest = format!("{REPO_PATH}/releases/latest/download/");
        let tagged = format!("{REPO_PATH}/releases/download/v{}/", self.release.version);
        let (status, body) = if let Some(name) = path.strip_prefix(&latest) {
            headers.insert(
                "location".to_string(),
                format!("https://github.com{tagged}{name}"),
            );
            (302, Vec::new())
        } else if let Some(body) = path.strip_prefix(&tagged).and_then(|name| self.file(name)) {
            headers.insert(
                "content-type".to_string(),
                "application/octet-stream".to_string(),
            );
            (200, body)
        } else {
            (404, b"Not Found".to_vec())
        };
        headers.insert("content-length".to_string(), body.len().to_string());
        Ok(Box::new(FakeResponse {
            status,
            headers,
            body,
            at: 0,
        }))
    }
}

/// A scratch folder under the temporary directory, removed on drop.
#[derive(Debug)]
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Scratch(std::env::temp_dir().join(format!(
            "mklm-client-update-{tag}-{}-{nanos}",
            std::process::id()
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
