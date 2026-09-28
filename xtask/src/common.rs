//! What several commands share: their dependencies ([`Env`]), the trust anchors of a tag, the
//! main-then-alternate verification of design m5b C.3, file helpers.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, bail};
use mklm_update::fetch::Transport;
use mklm_update::keys::{ANCHORS_REPO_PATH, parse_anchors};
use mklm_update::{
    Arch, KeyError, Purpose, Sha256Digest, SignatureSlot, TrustAnchors, TrustState, UpdateRefusal,
    VerifiedManifest, VerifyInput, Version, tries_alternate, verify_manifest,
};

use crate::host::{Clock, ReleaseHost, Repo};
use crate::releases::stable_tag_version;

/// Everything a command reaches outside the process.
pub struct Env<'a> {
    pub host: &'a mut dyn ReleaseHost,
    pub repo: &'a mut dyn Repo,
    pub clock: &'a mut dyn Clock,
    /// The production fetch path (WinHTTP in the tool, a fake in tests).
    pub transport: &'a mut dyn Transport,
    pub out: &'a mut dyn Write,
}

/// `vX.Y.Z` of a stable release; a pre-release tag is refused (design m5b B.3 step 1, B.5).
pub fn release_tag_version(tag: &str) -> anyhow::Result<Version> {
    if tag.contains('-') {
        bail!(
            "{tag} is a pre-release tag: pre-releases are not signed and carry no latest.json (design m5b B.5)"
        );
    }
    stable_tag_version(tag).with_context(|| format!("{tag:?} is not a tag of the form vX.Y.Z"))
}

/// The anchors file of a tag, `None` when the tag has none. A tag missing from the local
/// repository is an error, never "no anchors": the checks would silently skip it.
pub fn anchors_text_at(repo: &mut dyn Repo, tag: &str) -> anyhow::Result<Option<String>> {
    if repo.tag_commit(tag)?.is_none() {
        bail!("the tag {tag} is not in the local repository (git fetch --tags)");
    }
    repo.show(tag, ANCHORS_REPO_PATH)
}

/// The trust anchors a tag's build embeds; `None` when the tag has no anchors file or a file
/// without keys (a build that cannot update at all, such as v0.1.0).
pub fn anchors_at(repo: &mut dyn Repo, tag: &str) -> anyhow::Result<Option<TrustAnchors>> {
    let Some(text) = anchors_text_at(repo, tag)? else {
        return Ok(None);
    };
    let file = parse_anchors(&text).with_context(|| format!("{tag}:{ANCHORS_REPO_PATH}"))?;
    match TrustAnchors::from_file(&file) {
        Ok(anchors) => Ok(Some(anchors)),
        Err(KeyError::NotConfigured) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("{tag}:{ANCHORS_REPO_PATH}")),
    }
}

/// The signatures of one manifest.
#[derive(Debug, Clone, Copy)]
pub struct Signatures<'a> {
    pub main: &'a [u8],
    pub alt: Option<&'a [u8]>,
}

/// Design m5b C.3 with the alternate signature: the main one, then the alternate one when the
/// main one failed with `UnknownKey` / `RevokedKey`. On failure the main signature's refusal.
#[allow(clippy::too_many_arguments)]
pub fn verify_with_alternate(
    manifest: &[u8],
    signatures: Signatures<'_>,
    anchors: &TrustAnchors,
    installed: &Version,
    arch: Arch,
    now_unix: u64,
    tag: Option<&str>,
) -> Result<(VerifiedManifest, SignatureSlot), UpdateRefusal> {
    let state = TrustState::default();
    let verify = |signature: &[u8]| {
        verify_manifest(&VerifyInput {
            manifest,
            signature,
            anchors,
            state: &state,
            installed,
            arch,
            now_unix,
            tag,
            purpose: Purpose::Check,
        })
    };
    match verify(signatures.main) {
        Ok(verified) => Ok((verified, SignatureSlot::Main)),
        Err(error) if tries_alternate(&error) => match signatures.alt {
            Some(alt) => verify(alt)
                .map(|verified| (verified, SignatureSlot::Alt))
                .map_err(|_| error),
            None => Err(error),
        },
        Err(error) => Err(error),
    }
}

/// The trusted comment of a minisign signature file (its third line), for comparisons.
pub fn trusted_comment_of(signature: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(signature).ok()?;
    let line = text.lines().nth(2)?;
    line.strip_prefix("trusted comment: ").map(str::to_string)
}

/// A file's SHA-256 and size.
pub fn hash_file(path: &Path) -> anyhow::Result<(Sha256Digest, u64)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok((Sha256Digest::of(&bytes), bytes.len() as u64))
}

pub fn read(path: &Path) -> anyhow::Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

pub fn read_text(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

pub fn write(path: &Path, content: impl AsRef<[u8]>) -> anyhow::Result<()> {
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// The text of a file that ends with one line break, without it.
pub fn one_line(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

/// A path as the maintainer typed it, for the printed commands.
pub fn shown(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(
            release_tag_version("v0.2.1").unwrap(),
            Version::new(0, 2, 1)
        );
        for bad in ["0.2.1", "v0.2.1-rc.1", "v0.2", "latest", "v00.2.1"] {
            assert!(release_tag_version(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn trusted_comments() {
        let signature = b"untrusted comment: x\nRUQ=\ntrusted comment: mklm-latest-json v1 version=0.2.1\nAAAA\n";
        assert_eq!(
            trusted_comment_of(signature).as_deref(),
            Some("mklm-latest-json v1 version=0.2.1")
        );
        assert_eq!(trusted_comment_of(b"a\nb\n"), None);
        assert_eq!(one_line("x\r\n"), "x");
        assert_eq!(one_line("x\n"), "x");
        assert_eq!(one_line("x"), "x");
    }
}
