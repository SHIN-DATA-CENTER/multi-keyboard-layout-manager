//! `publish --tag vX.Y.Z --dir <dir>` (design m5b B.3; OPS-UX-TEST-2): verifies the offline
//! signatures for every release in the window, uploads them to the draft, checks the draft's
//! files, publishes it and checks what GitHub then serves.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use mklm_update::{Arch, Manifest, TrustAnchors, installer_name};
use serde_json::Value;

use crate::common::{
    Env, Signatures, anchors_at, hash_file, one_line, read, read_text, release_tag_version,
    trusted_comment_of, verify_with_alternate,
};
use crate::releases::stable_tag_version;
use crate::remote::{RemoteChecks, verify_remote};

/// How long `publish` waits for GitHub's CDN after publishing (B.3 step 6).
const AFTER_PUBLISH_WAIT: Duration = Duration::from_secs(5 * 60);
const AFTER_PUBLISH_RETRY: Duration = Duration::from_secs(30);

/// The runbook printed when the check after publishing fails (design m5b B.5).
const INCIDENT_RUNBOOK: &str = "\
The release is published but GitHub does not serve it as expected. Follow the release incident
runbook of docs/maintainer/release-signing.ja.md:
  1. gh release edit <tag> --prerelease   (the previous release becomes the latest again)
  2. cargo xtask verify --remote           (the previous release is served)
  3. Put \"Do not use this version\" at the top of the release notes.
  4. Release X.Y.(Z+1) the usual way (tags cannot be reused).";

/// What `prepare-release` recorded.
struct Prepared {
    commit: String,
    /// Installer name → (sha256 hex, GitHub digest).
    installers: Vec<(String, String, String)>,
    /// The window's tags and `--allow-strand`.
    targets: Vec<String>,
    allowed: Vec<String>,
}

fn read_prepared(path: &Path, tag: &str) -> anyhow::Result<Prepared> {
    let json: Value = serde_json::from_str(&read_text(path)?)
        .with_context(|| format!("{} is not JSON", path.display()))?;
    if json["tag"].as_str() != Some(tag) {
        bail!(
            "{} was prepared for {}, not {tag}",
            path.display(),
            json["tag"]
        );
    }
    let text = |value: &Value, what: &str| {
        value
            .as_str()
            .map(str::to_string)
            .with_context(|| format!("prepare.json: no {what}"))
    };
    let installers = json["assets"]
        .as_array()
        .context("prepare.json: no assets")?
        .iter()
        .map(|asset| {
            Ok((
                text(&asset["name"], "asset name")?,
                text(&asset["sha256"], "asset sha256")?,
                text(&asset["digest"], "asset digest")?,
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let strings = |value: &Value, field: &str| -> anyhow::Result<Vec<String>> {
        value
            .as_array()
            .with_context(|| format!("prepare.json: no {field}"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .or_else(|| item["tag"].as_str())
                    .map(str::to_string)
                    .with_context(|| format!("prepare.json: a malformed {field} entry"))
            })
            .collect()
    };
    Ok(Prepared {
        commit: text(&json["commit"], "commit")?,
        installers,
        targets: strings(&json["window"], "window")?,
        allowed: strings(&json["allow_strand"], "allow_strand")?,
    })
}

/// `publish` (design m5b B.3 steps 1–6). `anchors` of this tree verify the served manifest after
/// publishing.
pub fn publish(env: &mut Env<'_>, tag: &str, dir: &Path) -> anyhow::Result<()> {
    // 1. A stable tag, prepared, and nothing changed on GitHub since.
    let version = release_tag_version(tag)?;
    let prepared = read_prepared(&dir.join("prepare.json"), tag)?;
    let remote = env.host.tag_commit(tag)?;
    if remote != prepared.commit {
        bail!(
            "the tag {tag} is now {remote}; it was {} when prepared",
            prepared.commit
        );
    }
    let view = env.host.release(tag)?;
    if !view.is_draft || view.is_prerelease {
        bail!("the release {tag} is no longer a draft (or is a pre-release)");
    }
    let installer_names: Vec<String> = [Arch::X64, Arch::Arm64]
        .iter()
        .map(|arch| installer_name(&version, *arch))
        .collect();
    let before: BTreeSet<String> = view.assets.iter().map(|asset| asset.name.clone()).collect();
    let prepared_names: BTreeSet<String> = installer_names
        .iter()
        .cloned()
        .chain(["SHA256SUMS".to_string()])
        .collect();
    if before != prepared_names || view.assets.len() != prepared_names.len() {
        bail!("the draft's files are {before:?}, expected {prepared_names:?}");
    }
    for (name, _, digest) in &prepared.installers {
        let now = view
            .assets
            .iter()
            .find(|asset| &asset.name == name)
            .and_then(|asset| asset.digest.clone());
        if now.as_deref() != Some(digest.as_str()) {
            bail!("{name} changed on GitHub since prepare-release ({now:?}, was {digest})");
        }
    }

    // 2. The signatures, for every release in the window and this one.
    let manifest_path = dir.join(mklm_update::MANIFEST_NAME);
    let main_path = dir.join(mklm_update::SIGNATURE_NAME);
    let alt_path = dir.join(mklm_update::ALT_SIGNATURE_NAME);
    let manifest = read(&manifest_path)?;
    let main = read(&main_path)?;
    let alt = if alt_path.is_file() {
        Some(read(&alt_path)?)
    } else {
        None
    };
    let expected_comment = read_text(&dir.join("trusted-comment.txt"))?;
    let expected_comment = one_line(&expected_comment);
    for (name, signature) in [("main", Some(&main)), ("alternate", alt.as_ref())] {
        let Some(signature) = signature else { continue };
        let comment = trusted_comment_of(signature)
            .with_context(|| format!("the {name} signature file is malformed"))?;
        if comment != expected_comment {
            bail!(
                "the {name} signature's trusted comment is {comment:?}, prepare-release wrote \
                 {expected_comment:?}"
            );
        }
    }
    let parsed =
        Manifest::parse(&manifest).map_err(|refusal| anyhow::anyhow!("latest.json: {refusal}"))?;
    if parsed.version != version.to_string() {
        bail!("latest.json is for {}, not {tag}", parsed.version);
    }
    for (name, sha256, _) in &prepared.installers {
        if !parsed
            .assets
            .iter()
            .any(|asset| &asset.name == name && &asset.sha256 == sha256)
        {
            bail!("latest.json does not carry the prepared SHA-256 of {name}");
        }
    }
    let now = env.clock.now();
    let mut targets: Vec<String> = prepared
        .targets
        .iter()
        .filter(|target| !prepared.allowed.contains(target) && target.as_str() != tag)
        .cloned()
        .collect();
    targets.push(tag.to_string());
    for target in &targets {
        let Some(anchors) = anchors_at(env.repo, target)? else {
            writeln!(env.out, "note: {target} has no trust anchors; not checked")?;
            continue;
        };
        let installed =
            stable_tag_version(target).with_context(|| format!("{target} is not a release tag"))?;
        let (verified, slot) = verify_with_alternate(
            &manifest,
            Signatures {
                main: &main,
                alt: alt.as_deref(),
            },
            &anchors,
            &installed,
            Arch::X64,
            now,
            Some(tag),
        )
        .map_err(|refusal| {
            anyhow::anyhow!(
                "{target} would refuse this latest.json: {refusal}; nothing was uploaded"
            )
        })?;
        writeln!(
            env.out,
            "{target}: accepts it ({slot:?} signature, key {})",
            verified.signer
        )?;
    }

    // 3. Upload.
    let mut files: Vec<PathBuf> = vec![manifest_path.clone(), main_path.clone()];
    if alt.is_some() {
        files.push(alt_path.clone());
    }
    env.host.upload(tag, &files)?;

    // 4. The draft's files: exactly the planned five (six), the uploaded ones as on disk.
    let view = env.host.release(tag)?;
    let mut expected: BTreeSet<String> = prepared_names.clone();
    for file in &files {
        expected.insert(file_name(file));
    }
    let after: BTreeSet<String> = view.assets.iter().map(|asset| asset.name.clone()).collect();
    if after != expected || view.assets.len() != expected.len() {
        bail!(
            "after uploading, the draft's files are {after:?}, expected {expected:?}; not published"
        );
    }
    for file in &files {
        let (digest, _) = hash_file(file)?;
        let name = file_name(file);
        let github = view
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .and_then(|asset| asset.digest.clone());
        if github != Some(format!("sha256:{}", digest.to_hex())) {
            bail!("{name} on GitHub differs from the local file; not published");
        }
    }

    // 5. Publish.
    env.host.publish(tag)?;
    writeln!(env.out, "Published {tag}. Checking what GitHub serves...")?;

    // 6. The check after publishing, while the CDN catches up.
    let anchors = anchors_at(env.repo, tag)?.context("this release embeds no trust anchors")?;
    check_after_publishing(env, &anchors, &version)
}

fn check_after_publishing(
    env: &mut Env<'_>,
    anchors: &TrustAnchors,
    version: &mklm_update::Version,
) -> anyhow::Result<()> {
    let checks = RemoteChecks {
        installers: true,
        expect_version: Some(version.clone()),
        ..RemoteChecks::default()
    };
    let mut waited = Duration::ZERO;
    loop {
        match verify_remote(env, anchors, &checks) {
            Ok(served) => {
                writeln!(
                    env.out,
                    "publish: ok. GitHub serves MKLM {} (tag {}, {:?} signature).",
                    served.verified.version,
                    served.tag.as_deref().unwrap_or("unknown"),
                    served.slot
                )?;
                return Ok(());
            }
            Err(error) if waited < AFTER_PUBLISH_WAIT => {
                writeln!(env.out, "not yet ({error:#}); retrying in 30 seconds")?;
                env.clock.sleep(AFTER_PUBLISH_RETRY);
                waited += AFTER_PUBLISH_RETRY;
            }
            Err(error) => {
                writeln!(env.out, "{INCIDENT_RUNBOOK}")?;
                return Err(error.context("the check after publishing failed"));
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}
