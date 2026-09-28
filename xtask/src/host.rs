//! What xtask asks of GitHub and of git, behind traits that the tests replace with fakes: the
//! `gh` command ([`GhCli`], design m5b B.3) and the `git` command ([`GitCli`]). Neither is ever
//! given a secret: `gh` uses its own login (or `GH_TOKEN` in the canary workflow).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::Value;

use crate::releases::PublishedRelease;
use crate::time::{parse_http_date, parse_rfc3339_utc};

/// `owner/repo` of the product (from `mklm_update::REPO_URL`).
pub fn repo_slug() -> &'static str {
    mklm_update::REPO_URL
        .strip_prefix("https://github.com/")
        .unwrap_or(mklm_update::REPO_URL)
}

/// One asset of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetInfo {
    pub name: String,
    pub size: u64,
    /// GitHub's `digest` of the asset (`sha256:<hex>`), when GitHub reports one.
    pub digest: Option<String>,
}

/// A release, draft or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseView {
    pub tag: String,
    pub name: String,
    pub is_draft: bool,
    pub is_prerelease: bool,
    pub assets: Vec<AssetInfo>,
}

/// GitHub, through `gh` (design m5b B.3).
pub trait ReleaseHost {
    /// The commit the GitHub tag points at (annotated tags followed).
    fn tag_commit(&mut self, tag: &str) -> anyhow::Result<String>;
    /// The release of `tag` (drafts included) with its assets and their digests.
    fn release(&mut self, tag: &str) -> anyhow::Result<ReleaseView>;
    /// Downloads one asset of the release into `dir`; returns its path.
    fn download(&mut self, tag: &str, name: &str, dir: &Path) -> anyhow::Result<PathBuf>;
    /// `gh attestation verify` with the workflow, commit, tag and hosted runners of design m5b
    /// B.3 step 6.
    fn verify_attestation(&mut self, file: &Path, commit: &str, tag: &str) -> anyhow::Result<()>;
    /// The `Date` of `gh api -i /` (the trusted time, B.3 step 7).
    fn server_time(&mut self) -> anyhow::Result<u64>;
    /// `gh release list --exclude-drafts`.
    fn releases(&mut self) -> anyhow::Result<Vec<PublishedRelease>>;
    /// `gh release upload` (never overwriting).
    fn upload(&mut self, tag: &str, files: &[PathBuf]) -> anyhow::Result<()>;
    /// `gh release edit --draft=false --latest --title "MKLM <tag>"`.
    fn publish(&mut self, tag: &str) -> anyhow::Result<()>;
}

/// The local repository, through `git` (design m5b B.3).
pub trait Repo {
    /// `git rev-parse HEAD`.
    fn head(&mut self) -> anyhow::Result<String>;
    /// The commit of a local tag, `None` when there is no such tag.
    fn tag_commit(&mut self, tag: &str) -> anyhow::Result<Option<String>>;
    /// No change to tracked files.
    fn is_clean(&mut self) -> anyhow::Result<bool>;
    /// `git rev-parse --is-shallow-repository`.
    fn is_shallow(&mut self) -> anyhow::Result<bool>;
    /// `git tag -l "v*"`.
    fn tags(&mut self) -> anyhow::Result<Vec<String>>;
    /// `git show <tag>:<path>`; `None` when the tag has no such file.
    fn show(&mut self, tag: &str, path: &str) -> anyhow::Result<Option<String>>;
}

/// The local clock (and waiting), replaced in tests.
pub trait Clock {
    /// Unix seconds.
    fn now(&mut self) -> u64;
    fn sleep(&mut self, duration: Duration);
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Runs a command; its standard output, or an error with its standard error.
fn run(program: &str, args: &[&str], cwd: Option<&Path>) -> anyhow::Result<(bool, String)> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .with_context(|| format!("could not run {program} (is it installed and on PATH?)"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok((false, format!("{stdout}{stderr}")));
    }
    Ok((true, stdout))
}

fn must(program: &str, args: &[&str], cwd: Option<&Path>) -> anyhow::Result<String> {
    let (ok, text) = run(program, args, cwd)?;
    if !ok {
        bail!("{program} {} failed: {}", args.join(" "), text.trim());
    }
    Ok(text)
}

/// The `gh` command.
#[derive(Debug, Default)]
pub struct GhCli;

impl GhCli {
    fn gh(&self, args: &[&str]) -> anyhow::Result<String> {
        must("gh", args, None)
    }

    fn api_json(&self, endpoint: &str) -> anyhow::Result<Value> {
        let text = self.gh(&["api", endpoint])?;
        serde_json::from_str(&text).with_context(|| format!("gh api {endpoint}: not JSON"))
    }
}

impl ReleaseHost for GhCli {
    fn tag_commit(&mut self, tag: &str) -> anyhow::Result<String> {
        let slug = repo_slug();
        let mut object =
            self.api_json(&format!("repos/{slug}/git/ref/tags/{tag}"))?["object"].clone();
        // An annotated tag points at a tag object; follow it (a few levels at most).
        for _ in 0..5 {
            let kind = object["type"].as_str().unwrap_or_default().to_string();
            let sha = object["sha"]
                .as_str()
                .context("the tag reference has no object sha")?
                .to_string();
            match kind.as_str() {
                "commit" => return Ok(sha),
                "tag" => {
                    object =
                        self.api_json(&format!("repos/{slug}/git/tags/{sha}"))?["object"].clone();
                }
                other => bail!("tag {tag} points at a {other:?}, not a commit"),
            }
        }
        bail!("tag {tag}: too many levels of tag objects")
    }

    fn release(&mut self, tag: &str) -> anyhow::Result<ReleaseView> {
        let text = self.gh(&[
            "release",
            "view",
            tag,
            "--repo",
            repo_slug(),
            "--json",
            "tagName,name,isDraft,isPrerelease,apiUrl",
        ])?;
        let view: Value = serde_json::from_str(&text).context("gh release view: not JSON")?;
        let api_url = view["apiUrl"]
            .as_str()
            .context("gh release view: no apiUrl")?;
        let endpoint = api_url
            .strip_prefix("https://api.github.com/")
            .unwrap_or(api_url);
        // The REST release object has each asset's `digest` (design m5b B.3 step 5).
        let rest = self.api_json(endpoint)?;
        let assets = rest["assets"]
            .as_array()
            .context("the release has no asset list")?
            .iter()
            .map(|asset| {
                Ok(AssetInfo {
                    name: asset["name"]
                        .as_str()
                        .context("an asset without a name")?
                        .to_string(),
                    size: asset["size"].as_u64().context("an asset without a size")?,
                    digest: asset["digest"].as_str().map(str::to_string),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(ReleaseView {
            tag: view["tagName"].as_str().unwrap_or(tag).to_string(),
            name: view["name"].as_str().unwrap_or_default().to_string(),
            is_draft: view["isDraft"].as_bool().context("no isDraft")?,
            is_prerelease: view["isPrerelease"].as_bool().context("no isPrerelease")?,
            assets,
        })
    }

    fn download(&mut self, tag: &str, name: &str, dir: &Path) -> anyhow::Result<PathBuf> {
        let dir_text = dir.to_string_lossy().to_string();
        self.gh(&[
            "release",
            "download",
            tag,
            "--repo",
            repo_slug(),
            "--pattern",
            name,
            "--dir",
            &dir_text,
            "--clobber",
        ])?;
        let path = dir.join(name);
        if !path.is_file() {
            bail!("gh release download did not write {}", path.display());
        }
        Ok(path)
    }

    fn verify_attestation(&mut self, file: &Path, commit: &str, tag: &str) -> anyhow::Result<()> {
        let slug = repo_slug();
        let file_text = file.to_string_lossy().to_string();
        let workflow = format!("{slug}/.github/workflows/release.yml");
        let source_ref = format!("refs/tags/{tag}");
        self.gh(&[
            "attestation",
            "verify",
            &file_text,
            "--repo",
            slug,
            "--signer-workflow",
            &workflow,
            "--source-digest",
            commit,
            "--source-ref",
            &source_ref,
            "--deny-self-hosted-runners",
        ])
        .map(|_| ())
    }

    fn server_time(&mut self) -> anyhow::Result<u64> {
        let text = self.gh(&["api", "-i", "/"])?;
        date_header(&text).context("gh api -i / printed no valid Date header")
    }

    fn releases(&mut self) -> anyhow::Result<Vec<PublishedRelease>> {
        let text = self.gh(&[
            "release",
            "list",
            "--repo",
            repo_slug(),
            "--exclude-drafts",
            "--json",
            "tagName,publishedAt,isPrerelease",
            "--limit",
            "10000",
        ])?;
        parse_release_list(&text)
    }

    fn upload(&mut self, tag: &str, files: &[PathBuf]) -> anyhow::Result<()> {
        let files: Vec<String> = files
            .iter()
            .map(|file| file.to_string_lossy().to_string())
            .collect();
        let mut args = vec!["release", "upload", tag, "--repo", repo_slug()];
        args.extend(files.iter().map(String::as_str));
        self.gh(&args).map(|_| ())
    }

    fn publish(&mut self, tag: &str) -> anyhow::Result<()> {
        let title = format!("MKLM {tag}");
        self.gh(&[
            "release",
            "edit",
            tag,
            "--repo",
            repo_slug(),
            "--draft=false",
            "--latest",
            "--title",
            &title,
        ])
        .map(|_| ())
    }
}

/// The `Date` header of an HTTP response head as `gh api -i` prints it.
pub fn date_header(text: &str) -> Option<u64> {
    text.lines()
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("date"))
        .and_then(|(_, value)| parse_http_date(value.trim()))
}

/// The JSON of `gh release list --json tagName,publishedAt,isPrerelease`.
pub fn parse_release_list(text: &str) -> anyhow::Result<Vec<PublishedRelease>> {
    let list: Value = serde_json::from_str(text).context("gh release list: not JSON")?;
    list.as_array()
        .context("gh release list: not an array")?
        .iter()
        .map(|release| {
            let tag = release["tagName"]
                .as_str()
                .context("a release without tagName")?;
            let published = release["publishedAt"]
                .as_str()
                .context("a release without publishedAt")?;
            Ok(PublishedRelease {
                tag: tag.to_string(),
                published_at: parse_rfc3339_utc(published)
                    .with_context(|| format!("{tag}: publishedAt {published:?}"))?,
                is_prerelease: release["isPrerelease"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// The `git` command in the workspace.
#[derive(Debug)]
pub struct GitCli {
    root: PathBuf,
}

impl GitCli {
    pub fn new(root: PathBuf) -> GitCli {
        GitCli { root }
    }

    fn git(&self, args: &[&str]) -> anyhow::Result<String> {
        must("git", args, Some(&self.root))
    }
}

impl Repo for GitCli {
    fn head(&mut self) -> anyhow::Result<String> {
        Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    fn tag_commit(&mut self, tag: &str) -> anyhow::Result<Option<String>> {
        let spec = format!("refs/tags/{tag}^{{commit}}");
        let (ok, text) = run(
            "git",
            &["rev-parse", "--verify", "--quiet", &spec],
            Some(&self.root),
        )?;
        Ok(ok.then(|| text.trim().to_string()))
    }

    fn is_clean(&mut self) -> anyhow::Result<bool> {
        Ok(self
            .git(&["status", "--porcelain", "--untracked-files=no"])?
            .trim()
            .is_empty())
    }

    fn is_shallow(&mut self) -> anyhow::Result<bool> {
        Ok(self.git(&["rev-parse", "--is-shallow-repository"])?.trim() == "true")
    }

    fn tags(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .git(&["tag", "-l", "v*"])?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    fn show(&mut self, tag: &str, path: &str) -> anyhow::Result<Option<String>> {
        let object = format!("refs/tags/{tag}:{path}");
        let (exists, _) = run("git", &["cat-file", "-e", &object], Some(&self.root))?;
        if !exists {
            return Ok(None);
        }
        let text = self.git(&["show", &object])?;
        Ok(Some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository() {
        assert_eq!(
            repo_slug(),
            "SHIN-DATA-CENTER/multi-keyboard-layout-manager"
        );
    }

    #[test]
    fn date_headers() {
        let head = "HTTP/2.0 200 OK\r\nContent-Type: application/json\r\nDate: Thu, 15 Oct 2026 00:00:00 GMT\r\n\r\n{\"date\": \"x\"}";
        assert_eq!(date_header(head), Some(1_792_022_400));
        assert_eq!(
            date_header("HTTP/2.0 200 OK\n\nDate: Thu, 15 Oct 2026 00:00:00 GMT"),
            None
        );
        assert_eq!(date_header("HTTP/2.0 200 OK\ndate: nonsense\n"), None);
    }

    #[test]
    fn release_lists() {
        let text = r#"[{"isPrerelease":false,"publishedAt":"2026-10-15T00:00:00Z","tagName":"v0.2.0"},
                      {"isPrerelease":true,"publishedAt":"2026-11-01T12:00:00Z","tagName":"v0.3.0-rc.1"}]"#;
        let releases = parse_release_list(text).unwrap();
        assert_eq!(
            releases,
            vec![
                PublishedRelease {
                    tag: "v0.2.0".to_string(),
                    published_at: 1_792_022_400,
                    is_prerelease: false
                },
                PublishedRelease {
                    tag: "v0.3.0-rc.1".to_string(),
                    published_at: 1_792_022_400 + 17 * 86_400 + 12 * 3600,
                    is_prerelease: true
                },
            ]
        );
        assert!(parse_release_list("{}").is_err());
        assert!(parse_release_list(r#"[{"tagName":"v1.0.0","publishedAt":"yesterday"}]"#).is_err());
    }
}
