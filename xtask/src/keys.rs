//! The key commands on public data: `check-keys` (release.yml), `pubkey-line` (B.5 key
//! generation) and `verify-signer` (the official minisign release, B.1, B.5).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use mklm_update::keys::{
    ANCHORS_REPO_PATH, AnchorsFile, parse_anchors, public_key_fingerprint, public_key_id,
};
use mklm_update::verify::verify_file_signature;
use mklm_update::{KeyFingerprint, KeyId, KeyRole, Sha256Digest, TrustAnchors, Version};

use crate::common::{hash_file, read, read_text};
use crate::host::Repo;

/// The public key of minisign's author, Frank Denis (minisign's README; design m5b B.1).
pub const MINISIGN_AUTHOR_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";

/// `check-keys` (design m5b B.3): the committed anchors file of this tree, then every earlier
/// `v*` tag's file. Never skips what it cannot read.
pub fn check_keys(
    repo: &mut dyn Repo,
    current_text: &str,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let current = parse_anchors(current_text).context(ANCHORS_REPO_PATH)?;
    let anchors = TrustAnchors::from_file(&current).with_context(|| {
        format!(
            "{ANCHORS_REPO_PATH}: no usable keys (a release needs one primary and one backup key)"
        )
    })?;
    anchors.check_release_roles().context(ANCHORS_REPO_PATH)?;

    if repo.is_shallow()? {
        bail!("the repository is shallow: earlier tags cannot be read (fetch with fetch-depth: 0)");
    }
    let mut tags: Vec<(Version, String)> = Vec::new();
    for tag in repo.tags()? {
        match tag.strip_prefix('v').map(Version::parse) {
            Some(Ok(version)) if version.build.is_empty() => tags.push((version, tag)),
            _ => writeln!(out, "note: {tag} is not a version tag; skipped")?,
        }
    }
    if tags.is_empty() {
        bail!("no v* tag found: earlier trust anchors cannot be checked");
    }
    tags.sort_by(|(a, _), (b, _)| a.cmp_precedence(b));

    let mut seen_file = false;
    let mut checked = 0;
    for (_, tag) in &tags {
        let Some(text) = repo.show(tag, ANCHORS_REPO_PATH)? else {
            if seen_file {
                bail!(
                    "{tag} has no {ANCHORS_REPO_PATH}, although an earlier tag has one: cannot \
                     check it"
                );
            }
            writeln!(
                out,
                "{tag}: no trust anchors (a release without the updater); skipped"
            )?;
            continue;
        };
        seen_file = true;
        let past = parse_anchors(&text).with_context(|| format!("{tag}:{ANCHORS_REPO_PATH}"))?;
        compare_with_past(&current, &past, tag)?;
        checked += 1;
    }
    writeln!(out, "check-keys: ok")?;
    for (id, role) in anchors.ids() {
        writeln!(out, "  {} {id}", role.as_str())?;
    }
    for id in anchors.revoked_ids() {
        writeln!(out, "  revoked {id}")?;
    }
    writeln!(
        out,
        "  compared with the trust anchors of {checked} earlier tag(s)"
    )?;
    Ok(())
}

/// A key ID reused for another key, or a revoked ID embedded again.
fn compare_with_past(current: &AnchorsFile, past: &AnchorsFile, tag: &str) -> anyhow::Result<()> {
    for entry in &current.keys {
        if let Some(old) = past.keys.iter().find(|old| old.id == entry.id)
            && old.public_key != entry.public_key
        {
            bail!(
                "key ID {} names another public key in {tag}: a key ID may never be reused",
                entry.id
            );
        }
        if past.revoked.contains(&entry.id) {
            bail!(
                "key {} was revoked in {tag} and may not be embedded again",
                entry.id
            );
        }
    }
    Ok(())
}

/// The line of the trust anchors file for an official `minisign -G` public key file, its key ID
/// and fingerprint (design m5b B.5).
pub fn pubkey_line(
    pub_file: &str,
    role: KeyRole,
) -> anyhow::Result<(String, KeyId, KeyFingerprint)> {
    let body = pub_file.strip_suffix('\n').unwrap_or(pub_file);
    let lines: Vec<&str> = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let [comment, key] = lines.as_slice() else {
        bail!("a minisign public key file has two lines (a comment and the key)");
    };
    if !comment.starts_with("untrusted comment: ") {
        bail!("the first line is not a minisign comment: {comment:?}");
    }
    let id = public_key_id(key).context("the second line is not a minisign public key")?;
    let fingerprint = public_key_fingerprint(key)?;
    // The comment names the ID (`minisign public key <ID>`); minisign may print it without
    // leading zeros. A different ID means the wrong file.
    if let Some(stated) = comment
        .contains("public key")
        .then(|| comment.split_whitespace().last())
        .flatten()
        .map(|stated| stated.trim_end_matches(':'))
        && let Ok(value) = u64::from_str_radix(stated, 16)
        && stated.len() <= 16
        && value != u64::from_le_bytes(id.0)
    {
        bail!("the comment names key {stated}, but the key is {id}");
    }
    Ok((format!("{} {id} {key}", role.as_str()), id, fingerprint))
}

pub fn pubkey_line_command(path: &Path, role: KeyRole, out: &mut dyn Write) -> anyhow::Result<()> {
    let (line, id, fingerprint) = pubkey_line(&read_text(path)?, role)?;
    writeln!(out, "{line}")?;
    writeln!(out)?;
    writeln!(out, "key ID:      {id}")?;
    writeln!(
        out,
        "fingerprint: {} (SHA-256 of the key)",
        fingerprint.to_hex()
    )?;
    writeln!(
        out,
        "Paste the first line into {ANCHORS_REPO_PATH}, run `cargo xtask check-keys`, and check \
         that the key ID matches what `minisign -G` printed."
    )?;
    Ok(())
}

/// The zip's signature by minisign's author; returns the trusted comment. Legacy signatures are
/// accepted here (design m5b L).
pub fn verify_zip(zip: &[u8], signature: &[u8], author_key: &str) -> anyhow::Result<String> {
    verify_file_signature(author_key, zip, signature, true).map_err(|refusal| {
        anyhow::anyhow!("the zip's signature does not verify with minisign's author key: {refusal}")
    })
}

/// `verify-signer --zip --sig` (design m5b B.3, B.5).
pub fn verify_signer(zip_path: &Path, sig_path: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let zip = read(zip_path)?;
    let signature = read(sig_path)?;
    let comment = verify_zip(&zip, &signature, MINISIGN_AUTHOR_KEY)?;
    writeln!(
        out,
        "The signature of {} verifies with minisign's author key.",
        zip_path.display()
    )?;
    writeln!(out, "trusted comment: {comment}")?;
    writeln!(
        out,
        "SHA-256 of the zip:          {}",
        Sha256Digest::of(&zip).to_hex()
    )?;
    let (exe, size) = hash_exe_in_zip(zip_path)?;
    writeln!(
        out,
        "SHA-256 of minisign.exe:     {} ({size} bytes)",
        exe.to_hex()
    )?;
    writeln!(
        out,
        "Record both in docs/maintainer/release-signing.ja.md and keep the verified zip."
    )?;
    Ok(())
}

/// Extracts the zip with Windows' own `tar.exe` into a fresh temporary folder and hashes the one
/// `minisign.exe` in it.
fn hash_exe_in_zip(zip_path: &Path) -> anyhow::Result<(Sha256Digest, u64)> {
    let dir = std::env::temp_dir().join(format!(
        "mklm-xtask-verify-signer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = (|| {
        let tar = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("tar"));
        let status = Command::new(&tar)
            .arg("-xf")
            .arg(zip_path)
            .arg("-C")
            .arg(&dir)
            .status()
            .with_context(|| format!("running {}", tar.display()))?;
        if !status.success() {
            bail!("{} could not extract {}", tar.display(), zip_path.display());
        }
        let found = find_files(&dir, "minisign.exe")?;
        let [exe] = found.as_slice() else {
            bail!("the zip holds {} minisign.exe files, not one", found.len());
        };
        hash_file(exe)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn find_files(dir: &Path, name: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            found.extend(find_files(&path, name)?);
        } else if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            found.push(path);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::testing::{Key, anchors_text};

    /// A repository of tags and their anchors files.
    #[derive(Default)]
    struct Tags {
        shallow: bool,
        files: Vec<(String, Option<String>)>,
    }

    impl Repo for Tags {
        fn head(&mut self) -> anyhow::Result<String> {
            unreachable!()
        }
        fn tag_commit(&mut self, _: &str) -> anyhow::Result<Option<String>> {
            unreachable!()
        }
        fn is_clean(&mut self) -> anyhow::Result<bool> {
            unreachable!()
        }
        fn is_shallow(&mut self) -> anyhow::Result<bool> {
            Ok(self.shallow)
        }
        fn tags(&mut self) -> anyhow::Result<Vec<String>> {
            Ok(self.files.iter().map(|(tag, _)| tag.clone()).collect())
        }
        fn show(&mut self, tag: &str, path: &str) -> anyhow::Result<Option<String>> {
            assert_eq!(path, ANCHORS_REPO_PATH);
            let files: HashMap<_, _> = self.files.iter().cloned().collect();
            Ok(files.get(tag).cloned().flatten())
        }
    }

    fn run(repo: &mut Tags, current: &str) -> anyhow::Result<String> {
        let mut out = Vec::new();
        check_keys(repo, current, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn check_keys_passes_a_consistent_history() {
        let (p1, b1, p2) = (Key::new(), Key::new(), Key::new());
        let v020 = anchors_text(&[(KeyRole::Primary, &p1), (KeyRole::Backup, &b1)], &[]);
        let current = anchors_text(&[(KeyRole::Primary, &p2), (KeyRole::Backup, &b1)], &[&p1]);
        let mut repo = Tags {
            files: vec![
                ("v0.1.0".to_string(), None),
                ("v0.2.0".to_string(), Some(v020.clone())),
                ("v0.10.0".to_string(), Some(v020)),
            ],
            ..Tags::default()
        };
        let printed = run(&mut repo, &current).unwrap();
        assert!(printed.contains("v0.1.0: no trust anchors"), "{printed}");
        assert!(printed.contains("2 earlier tag(s)"), "{printed}");
    }

    #[test]
    fn check_keys_refusals() {
        let (p1, b1, p2, b2) = (Key::new(), Key::new(), Key::new(), Key::new());
        let good = anchors_text(&[(KeyRole::Primary, &p1), (KeyRole::Backup, &b1)], &[]);
        let history = |files: Vec<(&str, Option<String>)>| Tags {
            files: files
                .into_iter()
                .map(|(tag, file)| (tag.to_string(), file))
                .collect(),
            ..Tags::default()
        };
        // Roles.
        let two_primaries = anchors_text(&[(KeyRole::Primary, &p1), (KeyRole::Primary, &p2)], &[]);
        assert!(run(&mut history(vec![("v0.2.0", None)]), &two_primaries).is_err());
        let comments_only = "# MKLM update trust anchors (design m5b B.2).\n";
        assert!(run(&mut history(vec![("v0.2.0", None)]), comments_only).is_err());
        // A key ID naming another key in an earlier tag: the earlier tag's primary line with
        // the ID of today's key.
        let reused = format!(
            "primary {} {}\nbackup {} {}\n",
            p1.id, b2.public, b1.id, b1.public
        );
        assert!(run(&mut history(vec![("v0.2.0", Some(reused))]), &good).is_err());
        // A revoked ID embedded again.
        let revoked_p1 = anchors_text(&[(KeyRole::Primary, &p2), (KeyRole::Backup, &b1)], &[&p1]);
        assert!(run(&mut history(vec![("v0.3.0", Some(revoked_p1))]), &good).is_err());
        // Unreadable history: shallow, no tags, a later tag without the file.
        let mut shallow = history(vec![("v0.2.0", Some(good.clone()))]);
        shallow.shallow = true;
        assert!(run(&mut shallow, &good).is_err());
        assert!(run(&mut history(vec![]), &good).is_err());
        let gap = history(vec![("v0.2.0", Some(good.clone())), ("v0.3.0", None)]);
        let mut gap = gap;
        assert!(run(&mut gap, &good).is_err());
        // A malformed earlier file.
        assert!(
            run(
                &mut history(vec![("v0.2.0", Some("junk".to_string()))]),
                &good
            )
            .is_err()
        );
        // The consistent case passes.
        assert!(run(&mut history(vec![("v0.2.0", Some(good.clone()))]), &good).is_ok());
    }

    #[test]
    fn pubkey_lines_from_minisign_files() {
        let key = Key::new();
        // The form of `minisign -G` (the ID in the comment) and of the minisign crate (with a
        // colon).
        for comment in [
            format!("untrusted comment: minisign public key {}", key.id),
            format!("untrusted comment: minisign public key: {}", key.id),
            format!(
                "untrusted comment: minisign public key {:X}",
                u64::from_le_bytes(key.id.0)
            ),
        ] {
            let file = format!("{comment}\r\n{}\r\n", key.public);
            let (line, id, fingerprint) = pubkey_line(&file, KeyRole::Backup).unwrap();
            assert_eq!(line, format!("backup {} {}", key.id, key.public));
            assert_eq!(id, key.id);
            assert_eq!(fingerprint, public_key_fingerprint(&key.public).unwrap());
            // The line is what the anchors file takes.
            let parsed = parse_anchors(&line).unwrap();
            assert_eq!(parsed.keys[0].id, key.id);
        }
        let other = Key::new();
        let wrong_comment = format!(
            "untrusted comment: minisign public key {}\n{}\n",
            other.id, key.public
        );
        assert!(pubkey_line(&wrong_comment, KeyRole::Primary).is_err());
        assert!(pubkey_line(&key.public, KeyRole::Primary).is_err());
        assert!(pubkey_line("untrusted comment: x\nRWQ=\n", KeyRole::Primary).is_err());
        assert!(pubkey_line(&format!("x\n{}\n", key.public), KeyRole::Primary).is_err());
    }

    #[test]
    fn verify_signer_checks_the_authors_signature() {
        let author = Key::new();
        let zip = b"PK\x03\x04 a zip".to_vec();
        let signature = author.sign(&zip, "timestamp:1 file:minisign-0.12-win64.zip hashed");
        assert_eq!(
            verify_zip(&zip, &signature, &author.public).unwrap(),
            "timestamp:1 file:minisign-0.12-win64.zip hashed"
        );
        let mut changed = zip.clone();
        changed[5] ^= 1;
        assert!(verify_zip(&changed, &signature, &author.public).is_err());
        assert!(verify_zip(&zip, &Key::new().sign(&zip, "x"), &author.public).is_err());
        assert!(verify_zip(&zip, &signature, MINISIGN_AUTHOR_KEY).is_err());
        // The author's real key, with the prehashed test vector of minisign-verify.
        let vector = "untrusted comment: signature from minisign secret key\n\
                      RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
                      trusted comment: timestamp:1556193335\tfile:test\n\
                      y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==\n";
        assert_eq!(
            verify_zip(b"test", vector.as_bytes(), MINISIGN_AUTHOR_KEY).unwrap(),
            "timestamp:1556193335\tfile:test"
        );
        // And its legacy vector (accepted by verify-signer only).
        let legacy = "untrusted comment: signature from minisign secret key\n\
                      RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\n\
                      trusted comment: timestamp:1555779966\tfile:test\n\
                      QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==\n";
        assert!(verify_zip(b"test", legacy.as_bytes(), MINISIGN_AUTHOR_KEY).is_ok());
    }

    /// A stored (uncompressed) zip with one file, for Windows' tar.
    fn stored_zip(name: &str, content: &[u8]) -> Vec<u8> {
        let crc = crc32(content);
        let mut zip = Vec::new();
        let local = |zip: &mut Vec<u8>| {
            zip.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            zip.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            zip.extend_from_slice(&crc.to_le_bytes());
            zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
            zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
            zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(name.as_bytes());
            zip.extend_from_slice(content);
        };
        local(&mut zip);
        let central_offset = zip.len() as u32;
        zip.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&[0; 12]);
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(name.as_bytes());
        let central_len = zip.len() as u32 - central_offset;
        zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
        zip.extend_from_slice(&central_len.to_le_bytes());
        zip.extend_from_slice(&central_offset.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn the_executable_in_the_zip_is_hashed() {
        let content = b"MZ not really minisign".to_vec();
        let zip = stored_zip("minisign-win64/minisign.exe", &content);
        let path = std::env::temp_dir().join(format!("mklm-xtask-test-{}.zip", std::process::id()));
        std::fs::write(&path, &zip).unwrap();
        let result = hash_exe_in_zip(&path);
        let _ = std::fs::remove_file(&path);
        let (digest, size) = result.unwrap();
        assert_eq!(digest, Sha256Digest::of(&content));
        assert_eq!(size, content.len() as u64);
    }
}
