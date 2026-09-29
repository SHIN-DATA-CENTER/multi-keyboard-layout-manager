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

/// A path as the maintainer typed it, for messages.
pub fn shown(path: &Path) -> String {
    path.display().to_string()
}

/// A character PowerShell reads as a single quote (it ends a single-quoted string unless doubled).
fn is_single_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

/// One argument of a command that the maintainer pastes into PowerShell (`SIGN-OFFLINE.txt`, the
/// printed `key-drill check` line; design m5b B.3 step 12, F.6): bare when every character is
/// one PowerShell takes literally in a bare word (letters, digits, `\ / : . _ -`), otherwise in
/// single quotes, where PowerShell expands nothing (a quote character inside is doubled). The
/// fixed media paths and relative `--out` folders stay bare, as release-signing.ja.md shows them.
pub fn shell_arg(path: &Path) -> String {
    let text = path.display().to_string();
    let bare = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '\\' | '/' | ':' | '.' | '_' | '-'));
    if bare {
        return text;
    }
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for c in text.chars() {
        if is_single_quote(c) {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted.push('\'');
    quoted
}

/// The program of such a command: a quoted path is a string to PowerShell, which runs it only
/// after the call operator `&`.
pub fn shell_program(path: &Path) -> String {
    let arg = shell_arg(path);
    if arg.starts_with('\'') {
        format!("& {arg}")
    } else {
        arg
    }
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

    /// BUILD-RUN-2: paths with a space or a PowerShell character are quoted; the fixed forms of
    /// release-signing.ja.md stay as they are.
    #[test]
    fn command_arguments() {
        let arg = |text: &str| shell_arg(Path::new(text));
        let program = |text: &str| shell_program(Path::new(text));
        for bare in [
            r"E:\tools\minisign.exe",
            r"E:\mklm-keys\mklm-primary.key",
            r"release-work\v0.2.1\latest.json",
            r".\dist-dev\0.2.1\latest.json.minisig",
            r"C:\Users\me\mklm-dev-keys\minisign.exe",
            r"C:\Users\山田\mklm-dev-keys\minisign.exe",
        ] {
            assert_eq!(arg(bare), bare);
            assert_eq!(program(bare), bare);
        }
        assert_eq!(
            arg(r"D:\SHIN DATA CENTER\out\latest.json"),
            r"'D:\SHIN DATA CENTER\out\latest.json'"
        );
        assert_eq!(
            program(r"C:\Users\John Smith\minisign.exe"),
            r"& 'C:\Users\John Smith\minisign.exe'"
        );
        assert_eq!(arg(r"C:\it's\x"), r"'C:\it''s\x'");
        assert_eq!(arg("C:\\it\u{2019}s\\x"), "'C:\\it\u{2019}\u{2019}s\\x'");
        for special in [
            r"C:\$env:x\y",
            r"C:\a`b\y",
            r"C:\Program Files (x86)\y",
            r"C:\a;b\y",
            r"C:\a&b\y",
            r"C:\a,b\y",
            r"C:\a@b\y",
            r"C:\a#b\y",
            r"C:\RUNNER~1\y",
            "C:\\a\u{2013}b\\y",
        ] {
            assert_eq!(arg(special), format!("'{special}'"), "{special}");
        }
    }

    /// BUILD-RUN-2: PowerShell's own parser reads every quoted path back unchanged (nothing is
    /// run: only `Parser::ParseInput`).
    #[cfg(windows)]
    #[test]
    fn powershell_reads_the_quoted_paths_back() {
        let paths = [
            r"C:\Users\John Smith\mklm-dev-keys\minisign.exe",
            r"D:\SHIN DATA CENTER\mklm\dist-dev\space test\latest.json",
            r"C:\it's (x86)\$env:USERPROFILE\a`b;c&d,e@f#g\latest.json.minisig",
            r"release-work\v0.2.1\latest.json",
        ];
        let line = format!(
            "{} -S -m {} -x {} -t \"mklm-dev-latest-json v1 version=0.2.1 issued_at=1\"",
            shell_program(Path::new(paths[0])),
            shell_arg(Path::new(paths[1])),
            shell_arg(Path::new(paths[2])),
        );
        let elements = crate::testing::powershell_command_elements(&line);
        assert_eq!(
            elements,
            vec![vec![
                paths[0].to_string(),
                "-S".to_string(),
                "-m".to_string(),
                paths[1].to_string(),
                "-x".to_string(),
                paths[2].to_string(),
                "-t".to_string(),
                "mklm-dev-latest-json v1 version=0.2.1 issued_at=1".to_string(),
            ]],
            "{line}"
        );
        // A bare program stays bare; the relative path too.
        let line = format!(
            "{} -m {}",
            shell_program(Path::new(r"E:\tools\minisign.exe")),
            shell_arg(Path::new(paths[3]))
        );
        assert_eq!(
            crate::testing::powershell_command_elements(&line),
            vec![vec![
                r"E:\tools\minisign.exe".to_string(),
                "-m".to_string(),
                paths[3].to_string(),
            ]]
        );
    }
}
