//! `key-drill start` / `key-drill check` (design m5b B.6; OPS-UX-TEST-13): the yearly proof that
//! the backup key on its medium is the key the shipped builds embed, that the medium reads and
//! that the password is remembered. xtask writes a fresh nonce and the offline minisign command;
//! the maintainer signs offline; xtask checks the signature against every release in the window.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, bail};
use mklm_update::keys::{ANCHORS_REPO_PATH, parse_anchors, signature_key_id};
use mklm_update::verify::verify_file_signature;
use mklm_update::{KEY_DRILL_COMMENT_PREFIX, KeyId, KeyRole};

use crate::common::{Env, anchors_text_at, read, shown, write};
use crate::prepare::signing_media;
use crate::releases::window_targets;
use crate::time::format_utc;

const NONCE_NAME: &str = "nonce.bin";
const SIGNATURE_NAME: &str = "nonce.bin.minisig";
const NONCE_LEN: usize = 32;

/// 32 bytes nobody can predict: SipHash outputs under the process's random keys (the standard
/// library seeds them from the operating system), over a counter and the time. The drill needs a
/// file that was never signed before, not a secret.
fn nonce() -> [u8; NONCE_LEN] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let mut out = [0u8; NONCE_LEN];
    for (index, chunk) in out.chunks_mut(8).enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_usize(index);
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        chunk.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    out
}

/// `key-drill start --role <role> --out <dir>`.
pub fn start(role: KeyRole, dir: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let nonce_path = dir.join(NONCE_NAME);
    if nonce_path.exists() || dir.join(SIGNATURE_NAME).exists() {
        bail!(
            "{} already holds a drill: use a new folder for every drill",
            dir.display()
        );
    }
    write(&nonce_path, nonce())?;
    let (minisign, key_file) = signing_media(role);
    let command = format!(
        "{minisign} -S -s {key_file} -m {} -x {} -t \"{KEY_DRILL_COMMENT_PREFIX}\"\n",
        shown(&nonce_path),
        shown(&dir.join(SIGNATURE_NAME)),
    );
    write(&dir.join("SIGN-OFFLINE.txt"), &command)?;
    writeln!(out, "Wrote {} and SIGN-OFFLINE.txt.", shown(&nonce_path))?;
    writeln!(
        out,
        "Offline, with the {} key's medium, run:",
        role.as_str()
    )?;
    write!(out, "  {command}")?;
    writeln!(
        out,
        "then: cargo xtask key-drill check --dir {} --role {}",
        shown(dir),
        role.as_str()
    )?;
    Ok(())
}

/// `key-drill check --dir <dir> --role <role>`: writes nothing.
pub fn check(env: &mut Env<'_>, dir: &Path, role: KeyRole) -> anyhow::Result<KeyId> {
    let nonce = read(&dir.join(NONCE_NAME))?;
    if nonce.len() != NONCE_LEN {
        bail!("{NONCE_NAME} is not the {NONCE_LEN} bytes key-drill start wrote");
    }
    let signature = read(&dir.join(SIGNATURE_NAME))?;
    let text = std::str::from_utf8(&signature).context("the signature is not text")?;
    let key_id = signature_key_id(text).context("the signature file is malformed")?;
    let now = env.host.server_time()?;
    let releases = env.host.releases()?;
    let window = window_targets(&releases, now);
    let mut checked = Vec::new();
    for target in &window {
        let Some(anchors_text) = anchors_text_at(env.repo, &target.tag)? else {
            writeln!(
                env.out,
                "note: {} has no trust anchors; not checked",
                target.tag
            )?;
            continue;
        };
        let file = parse_anchors(&anchors_text)
            .with_context(|| format!("{}:{ANCHORS_REPO_PATH}", target.tag))?;
        let Some(entry) = file.keys.iter().find(|entry| entry.role == role) else {
            bail!("{} embeds no {} key", target.tag, role.as_str());
        };
        if entry.id != key_id {
            bail!(
                "the drill was signed with key {key_id}, but {} embeds {} {} (an old or wrong key \
                 file)",
                target.tag,
                role.as_str(),
                entry.id
            );
        }
        if file.revoked.contains(&key_id) {
            bail!("{} revokes key {key_id}", target.tag);
        }
        let comment = verify_file_signature(&entry.public_key, &nonce, &signature, false).map_err(
            |refusal| {
                anyhow::anyhow!(
                    "the signature of {NONCE_NAME} does not verify with the {} key of {}: \
                     {refusal} (was it made for another drill's nonce?)",
                    role.as_str(),
                    target.tag
                )
            },
        )?;
        if comment != KEY_DRILL_COMMENT_PREFIX {
            bail!("the trusted comment is {comment:?}, not {KEY_DRILL_COMMENT_PREFIX:?}");
        }
        checked.push(target.tag.clone());
    }
    if checked.is_empty() {
        bail!("no release in the window embeds trust anchors: nothing to check against");
    }
    writeln!(
        env.out,
        "key-drill: ok. {NONCE_NAME} is signed by the {} key {key_id} that every release in the \
         window embeds: {}",
        role.as_str(),
        checked.join(", ")
    )?;
    writeln!(
        env.out,
        "Record in docs/maintainer/release-signing.ja.md: {} | {key_id} | ok",
        format_utc(now)
    )?;
    Ok(key_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        DAY, FakeClock, FakeHost, FakeRepo, FakeWeb, Key, T0, TempDir, anchors_text,
    };

    #[test]
    fn nonces_differ() {
        assert_ne!(nonce(), nonce());
    }

    struct World {
        host: FakeHost,
        repo: FakeRepo,
        p: Key,
        b: Key,
    }

    fn world() -> World {
        let (p, b) = (Key::new(), Key::new());
        let anchors = anchors_text(&[(KeyRole::Primary, &p), (KeyRole::Backup, &b)], &[]);
        let mut repo = FakeRepo::default();
        repo.tag("v0.1.0", &"1".repeat(40), None);
        repo.tag("v0.2.0", &"2".repeat(40), Some(anchors.clone()));
        repo.tag("v0.3.0", &"3".repeat(40), Some(anchors));
        let mut host = FakeHost {
            time: T0 + 100 * DAY,
            ..FakeHost::default()
        };
        host.published_release("v0.1.0", T0 - 400 * DAY);
        host.published_release("v0.2.0", T0);
        host.published_release("v0.3.0", T0 + 50 * DAY);
        World { host, repo, p, b }
    }

    fn check_with(world: &mut World, dir: &Path, role: KeyRole) -> anyhow::Result<KeyId> {
        let mut clock = FakeClock::default();
        let mut web = FakeWeb::default();
        let mut out = Vec::new();
        let mut env = Env {
            host: &mut world.host,
            repo: &mut world.repo,
            clock: &mut clock,
            transport: &mut web,
            out: &mut out,
        };
        check(&mut env, dir, role)
    }

    #[test]
    fn a_drill_round_trip() {
        let mut world = world();
        let dir = TempDir::new("drill");
        let mut out = Vec::new();
        start(KeyRole::Backup, dir.path(), &mut out).unwrap();
        let command = std::fs::read_to_string(dir.path().join("SIGN-OFFLINE.txt")).unwrap();
        assert!(
            command.starts_with(
                r"F:\tools\minisign.exe -S -s F:\mklm-keys-backup\mklm-backup.key -m "
            ),
            "{command}"
        );
        assert!(
            command.trim_end().ends_with("-t \"mklm-key-drill v1\""),
            "{command}"
        );
        assert_eq!(command.lines().count(), 1);
        // A second start in the same folder is refused.
        assert!(start(KeyRole::Backup, dir.path(), &mut out).is_err());

        let nonce = std::fs::read(dir.path().join(NONCE_NAME)).unwrap();
        let signature_path = dir.path().join(SIGNATURE_NAME);
        std::fs::write(
            &signature_path,
            world.b.sign(&nonce, KEY_DRILL_COMMENT_PREFIX),
        )
        .unwrap();
        assert_eq!(
            check_with(&mut world, dir.path(), KeyRole::Backup).unwrap(),
            world.b.id
        );

        // The primary key is not the backup key.
        std::fs::write(
            &signature_path,
            world.p.sign(&nonce, KEY_DRILL_COMMENT_PREFIX),
        )
        .unwrap();
        assert!(check_with(&mut world, dir.path(), KeyRole::Backup).is_err());
        // An old backup key that no release embeds.
        std::fs::write(
            &signature_path,
            Key::new().sign(&nonce, KEY_DRILL_COMMENT_PREFIX),
        )
        .unwrap();
        assert!(check_with(&mut world, dir.path(), KeyRole::Backup).is_err());
        // Another drill's nonce.
        std::fs::write(
            &signature_path,
            world.b.sign(b"an older nonce", KEY_DRILL_COMMENT_PREFIX),
        )
        .unwrap();
        assert!(check_with(&mut world, dir.path(), KeyRole::Backup).is_err());
        // Another trusted comment.
        std::fs::write(&signature_path, world.b.sign(&nonce, "mklm-latest-json v1")).unwrap();
        assert!(check_with(&mut world, dir.path(), KeyRole::Backup).is_err());
    }

    #[test]
    fn every_release_in_the_window_must_embed_the_key() {
        let mut world = world();
        // v0.3.0 replaced the backup key: a drill with the old key fails, one with the new key
        // fails for v0.2.0 while v0.2.0 is in the window.
        let b2 = Key::new();
        let rotated = anchors_text(
            &[(KeyRole::Primary, &world.p), (KeyRole::Backup, &b2)],
            &[&world.b],
        );
        world.repo.tag("v0.3.0", &"3".repeat(40), Some(rotated));
        let dir = TempDir::new("drill-window");
        start(KeyRole::Backup, dir.path(), &mut Vec::new()).unwrap();
        let nonce = std::fs::read(dir.path().join(NONCE_NAME)).unwrap();
        let signature_path = dir.path().join(SIGNATURE_NAME);
        std::fs::write(
            &signature_path,
            world.b.sign(&nonce, KEY_DRILL_COMMENT_PREFIX),
        )
        .unwrap();
        assert!(check_with(&mut world, dir.path(), KeyRole::Backup).is_err());
        std::fs::write(&signature_path, b2.sign(&nonce, KEY_DRILL_COMMENT_PREFIX)).unwrap();
        assert!(check_with(&mut world, dir.path(), KeyRole::Backup).is_err());
        // 400 days after v0.3.0, v0.2.0 left the window: the new key passes.
        world.host.time = T0 + 50 * DAY + 400 * DAY;
        assert_eq!(
            check_with(&mut world, dir.path(), KeyRole::Backup).unwrap(),
            b2.id
        );
    }
}
