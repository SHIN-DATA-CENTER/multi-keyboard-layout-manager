//! Fakes of the tests: throwaway minisign keys (generated in the test process by the `minisign`
//! dev-dependency, never stored), a fake GitHub (`ReleaseHost`), a fake repository, a fake clock
//! and a fake web behind the production URLs (`Transport`). Nothing here touches the network.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail};
use mklm_update::fetch::{Response, Timeouts, Transport, TransportError};
use mklm_update::keys::ANCHORS_REPO_PATH;
use mklm_update::url::Url;
use mklm_update::{KeyId, KeyRole, REPO_URL, Sha256Digest};

use crate::host::{AssetInfo, Clock, ReleaseHost, ReleaseView, Repo};
use crate::releases::PublishedRelease;

/// A throwaway key pair.
pub struct Key {
    pair: minisign::KeyPair,
    pub id: KeyId,
    pub public: String,
}

impl Key {
    pub fn new() -> Key {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().expect("a throwaway key");
        let id = KeyId(pair.pk.keynum().try_into().expect("8 bytes"));
        let public = pair.pk.to_base64();
        Key { pair, id, public }
    }

    pub fn sign(&self, data: &[u8], trusted_comment: &str) -> Vec<u8> {
        minisign::sign(
            Some(&self.pair.pk),
            &self.pair.sk,
            Cursor::new(data),
            Some(trusted_comment),
            None,
        )
        .expect("sign")
        .into_string()
        .into_bytes()
    }

    /// The public key file `minisign -G` writes.
    pub fn pub_file(&self) -> String {
        format!(
            "untrusted comment: minisign public key {}\n{}\n",
            self.id, self.public
        )
    }
}

/// A trust anchors file.
pub fn anchors_text(keys: &[(KeyRole, &Key)], revoked: &[&Key]) -> String {
    let mut text = String::from("# MKLM update trust anchors (design m5b B.2).\n");
    for (role, key) in keys {
        text.push_str(&format!("{} {} {}\n", role.as_str(), key.id, key.public));
    }
    for key in revoked {
        text.push_str(&format!("revoked {}\n", key.id));
    }
    text
}

/// 2026-10-15 00:00 UTC.
pub const T0: u64 = 1_792_022_400;
pub const DAY: u64 = 86_400;

pub fn digest_text(bytes: &[u8]) -> String {
    format!("sha256:{}", Sha256Digest::of(bytes).to_hex())
}

/// GitHub as the tests script it.
#[derive(Default)]
pub struct FakeHost {
    pub tag_commits: HashMap<String, String>,
    pub views: HashMap<String, ReleaseView>,
    /// (tag, asset name) → content.
    pub files: HashMap<(String, String), Vec<u8>>,
    pub attestation_fails: bool,
    pub time: u64,
    pub published: Vec<PublishedRelease>,
    pub uploads: Vec<(String, Vec<String>)>,
    pub publishes: Vec<String>,
    /// Do not add uploaded files to the release (the upload check must fail).
    pub lose_uploads: bool,
}

impl FakeHost {
    /// A draft of `tag` with these assets (content known to `download`).
    pub fn draft(&mut self, tag: &str, commit: &str, assets: &[(&str, Vec<u8>)]) {
        self.tag_commits.insert(tag.to_string(), commit.to_string());
        self.views.insert(
            tag.to_string(),
            ReleaseView {
                tag: tag.to_string(),
                name: format!("MKLM {tag} — UNSIGNED, DO NOT PUBLISH"),
                is_draft: true,
                is_prerelease: false,
                assets: assets
                    .iter()
                    .map(|(name, content)| AssetInfo {
                        name: name.to_string(),
                        size: content.len() as u64,
                        digest: Some(digest_text(content)),
                    })
                    .collect(),
            },
        );
        for (name, content) in assets {
            self.files
                .insert((tag.to_string(), name.to_string()), content.clone());
        }
    }

    pub fn published_release(&mut self, tag: &str, published_at: u64) {
        self.published.push(PublishedRelease {
            tag: tag.to_string(),
            published_at,
            is_prerelease: false,
        });
    }
}

impl ReleaseHost for FakeHost {
    fn tag_commit(&mut self, tag: &str) -> anyhow::Result<String> {
        self.tag_commits
            .get(tag)
            .cloned()
            .ok_or_else(|| anyhow!("no tag {tag} on GitHub"))
    }

    fn release(&mut self, tag: &str) -> anyhow::Result<ReleaseView> {
        self.views
            .get(tag)
            .cloned()
            .ok_or_else(|| anyhow!("no release {tag}"))
    }

    fn download(&mut self, tag: &str, name: &str, dir: &Path) -> anyhow::Result<PathBuf> {
        let content = self
            .files
            .get(&(tag.to_string(), name.to_string()))
            .ok_or_else(|| anyhow!("no asset {name}"))?;
        std::fs::create_dir_all(dir)?;
        let path = dir.join(name);
        std::fs::write(&path, content)?;
        Ok(path)
    }

    fn verify_attestation(&mut self, _: &Path, _: &str, _: &str) -> anyhow::Result<()> {
        if self.attestation_fails {
            bail!("no attestation");
        }
        Ok(())
    }

    fn server_time(&mut self) -> anyhow::Result<u64> {
        Ok(self.time)
    }

    fn releases(&mut self) -> anyhow::Result<Vec<PublishedRelease>> {
        Ok(self.published.clone())
    }

    fn upload(&mut self, tag: &str, files: &[PathBuf]) -> anyhow::Result<()> {
        let names: Vec<String> = files
            .iter()
            .map(|file| file.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        self.uploads.push((tag.to_string(), names.clone()));
        if self.lose_uploads {
            return Ok(());
        }
        let view = self
            .views
            .get_mut(tag)
            .ok_or_else(|| anyhow!("no release {tag}"))?;
        for (file, name) in files.iter().zip(names) {
            if view.assets.iter().any(|asset| asset.name == name) {
                bail!("{name} exists (immutable)");
            }
            let content = std::fs::read(file)?;
            view.assets.push(AssetInfo {
                name,
                size: content.len() as u64,
                digest: Some(digest_text(&content)),
            });
        }
        Ok(())
    }

    fn publish(&mut self, tag: &str) -> anyhow::Result<()> {
        self.publishes.push(tag.to_string());
        if let Some(view) = self.views.get_mut(tag) {
            view.is_draft = false;
        }
        Ok(())
    }
}

/// A repository: HEAD, tags with their commits and anchors files.
#[derive(Default)]
pub struct FakeRepo {
    pub head: String,
    pub dirty: bool,
    pub shallow: bool,
    /// tag → (commit, anchors file).
    pub tags: BTreeMap<String, (String, Option<String>)>,
}

impl FakeRepo {
    pub fn tag(&mut self, tag: &str, commit: &str, anchors: Option<String>) {
        self.tags
            .insert(tag.to_string(), (commit.to_string(), anchors));
    }
}

impl Repo for FakeRepo {
    fn head(&mut self) -> anyhow::Result<String> {
        Ok(self.head.clone())
    }

    fn tag_commit(&mut self, tag: &str) -> anyhow::Result<Option<String>> {
        Ok(self.tags.get(tag).map(|(commit, _)| commit.clone()))
    }

    fn is_clean(&mut self) -> anyhow::Result<bool> {
        Ok(!self.dirty)
    }

    fn is_shallow(&mut self) -> anyhow::Result<bool> {
        Ok(self.shallow)
    }

    fn tags(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(self.tags.keys().cloned().collect())
    }

    fn show(&mut self, tag: &str, path: &str) -> anyhow::Result<Option<String>> {
        assert_eq!(path, ANCHORS_REPO_PATH);
        Ok(self.tags.get(tag).and_then(|(_, anchors)| anchors.clone()))
    }
}

#[derive(Debug, Default)]
pub struct FakeClock {
    pub now: u64,
    pub slept: Vec<Duration>,
}

impl Clock for FakeClock {
    fn now(&mut self) -> u64 {
        self.now
    }

    fn sleep(&mut self, duration: Duration) {
        self.slept.push(duration);
        self.now += duration.as_secs();
    }
}

/// Status, headers, body.
pub type Answer = (u16, Vec<(String, String)>, Vec<u8>);

/// The web behind the production URLs: full URL → answer. Unknown URLs 404.
#[derive(Default)]
pub struct FakeWeb {
    pub routes: HashMap<String, Answer>,
    pub requests: Vec<String>,
    /// Requests answered with a connection failure before the routes apply.
    pub fail_first: usize,
}

impl FakeWeb {
    pub fn file(&mut self, path: &str, body: &[u8]) {
        self.routes.insert(
            format!("{REPO_URL}{path}"),
            (200, Vec::new(), body.to_vec()),
        );
    }

    pub fn redirect(&mut self, path: &str, to_path: &str) {
        self.routes.insert(
            format!("{REPO_URL}{path}"),
            (
                302,
                vec![("Location".to_string(), format!("{REPO_URL}{to_path}"))],
                Vec::new(),
            ),
        );
    }

    /// A release served as GitHub serves the latest one: `latest/download/<name>` redirects to
    /// `download/<tag>/<name>`.
    pub fn latest_release(&mut self, tag: &str, files: &[(&str, &[u8])]) {
        for (name, body) in files {
            self.redirect(
                &format!("/releases/latest/download/{name}"),
                &format!("/releases/download/{tag}/{name}"),
            );
            self.file(&format!("/releases/download/{tag}/{name}"), body);
        }
    }
}

struct FakeResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    at: usize,
}

impl Response for FakeResponse {
    fn status(&self) -> u16 {
        self.status
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let n = (self.body.len() - self.at).min(buf.len());
        buf[..n].copy_from_slice(&self.body[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl Transport for FakeWeb {
    fn get(
        &mut self,
        url: &Url,
        _accept: &str,
        _timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError> {
        self.requests.push(url.as_str().to_string());
        if self.fail_first > 0 {
            self.fail_first -= 1;
            return Err(TransportError::CannotConnect);
        }
        let (status, headers, body) =
            self.routes
                .get(url.as_str())
                .cloned()
                .unwrap_or((404, Vec::new(), Vec::new()));
        Ok(Box::new(FakeResponse {
            status,
            headers,
            body,
            at: 0,
        }))
    }
}

/// A fresh temporary folder, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> TempDir {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("mklm-xtask-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Standard base64 (for `powershell -EncodedCommand`).
#[cfg(windows)]
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How PowerShell (Windows PowerShell 5.1, the maintainer's shell) reads pasted command lines:
/// for each line, the elements of its one command (the program, then each parameter and
/// argument) as values. Only `Parser::ParseInput` runs; the commands never do. Panics when a line
/// does not parse or is not exactly one command.
#[cfg(windows)]
pub fn powershell_command_elements(lines: &str) -> Vec<Vec<String>> {
    const SCRIPT: &str = r#"
$text = $env:MKLM_XTASK_TEST_COMMANDS
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
if ($errors.Count -gt 0) { [Console]::Out.WriteLine('PARSE-ERROR ' + $errors[0].Message); exit 3 }
foreach ($statement in $ast.EndBlock.Statements) {
  if (-not ($statement -is [System.Management.Automation.Language.PipelineAst]) -or $statement.PipelineElements.Count -ne 1) { [Console]::Out.WriteLine('NOT-ONE-COMMAND'); exit 4 }
  $command = $statement.PipelineElements[0]
  if (-not ($command -is [System.Management.Automation.Language.CommandAst])) { [Console]::Out.WriteLine('NOT-A-COMMAND'); exit 4 }
  foreach ($element in $command.CommandElements) {
    if ($element -is [System.Management.Automation.Language.StringConstantExpressionAst]) { $value = $element.Value }
    elseif ($element -is [System.Management.Automation.Language.CommandParameterAst]) { $value = $element.Extent.Text }
    else { $value = 'UNEXPECTED ' + $element.GetType().Name }
    [Console]::Out.WriteLine('E ' + $value)
  }
  [Console]::Out.WriteLine('END')
}
exit 0
"#;
    let utf16: Vec<u8> = SCRIPT
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    let powershell = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .unwrap_or_else(|| PathBuf::from("powershell.exe"));
    let output = std::process::Command::new(&powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &base64(&utf16),
        ])
        .env("MKLM_XTASK_TEST_COMMANDS", lines)
        .output()
        .unwrap_or_else(|error| panic!("running {}: {error}", powershell.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{lines}\n{stdout}");
    let mut commands = Vec::new();
    let mut current = Vec::new();
    for line in stdout.lines().map(|line| line.trim_end_matches('\r')) {
        if let Some(value) = line.strip_prefix("E ") {
            current.push(value.to_string());
        } else if line == "END" {
            commands.push(std::mem::take(&mut current));
        }
    }
    commands
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn base64_of_the_encoded_command() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
