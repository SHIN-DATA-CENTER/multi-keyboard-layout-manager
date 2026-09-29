//! `prepare-release` (design m5b B.3): every check before signing, then `latest.json`, the
//! trusted comment, the offline minisign command and `prepare.json`. And `prepare-release --dev`,
//! the debug-build rehearsal of design m5b F.6.
//!
//! Nothing here opens a secret key: `SIGN-OFFLINE.txt` names the key file for the official
//! minisign binary, which the maintainer runs offline (design m5b B.5; SECURITY-2).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, bail};
use mklm_update::fetch::{FetchError, Limits, fetch_alt_signature, fetch_manifest};
use mklm_update::url::Endpoints;
use mklm_update::version::parse_release_version;
use mklm_update::{
    Arch, DEFAULT_VALIDITY_DAYS, DEV_TRUSTED_COMMENT_PREFIX, KeyError, KeyId, KeyRole,
    MAX_VALIDITY_SECS, Manifest, ManifestAsset, TRUSTED_COMMENT_PREFIX, TrustAnchors, Version,
    installer_name,
};
use serde_json::json;

use crate::common::{
    Env, Signatures, anchors_at, hash_file, read_text, release_tag_version, shell_arg,
    shell_program, shown, verify_with_alternate, write,
};
use crate::keys::pubkey_line;
use crate::releases::{StrandRow, strand_row, verdict, window_targets};
use crate::sums;
use crate::time::{DAY, format_date, format_utc};

/// The fixed media of design m5b B.5: the primary key's USB stick and the backup key's medium.
const PRIMARY_MINISIGN: &str = r"E:\tools\minisign.exe";
const PRIMARY_KEY_FILE: &str = r"E:\mklm-keys\mklm-primary.key";
const BACKUP_MINISIGN: &str = r"F:\tools\minisign.exe";
const BACKUP_KEY_FILE: &str = r"F:\mklm-keys-backup\mklm-backup.key";

/// Largest difference between the local clock and GitHub's (B.3 step 7).
const MAX_CLOCK_SKEW: u64 = 5 * 60;

/// The minisign binary and key file of a key role.
pub fn signing_media(role: KeyRole) -> (&'static str, &'static str) {
    match role {
        KeyRole::Primary => (PRIMARY_MINISIGN, PRIMARY_KEY_FILE),
        KeyRole::Backup => (BACKUP_MINISIGN, BACKUP_KEY_FILE),
    }
}

/// The options of the production grammar.
#[derive(Debug, Clone)]
pub struct Prepare {
    pub tag: String,
    pub commit: String,
    pub main_key_id: KeyId,
    pub alt_key_id: Option<KeyId>,
    pub expires_days: u64,
    pub revoke: Vec<KeyId>,
    pub min_from_version: Option<String>,
    pub allow_strand: Vec<String>,
    pub published_misdated: bool,
    pub out: PathBuf,
}

/// The two installers of a version, x64 first.
fn installer_names(version: &Version) -> [(Arch, String); 2] {
    [
        (Arch::X64, installer_name(version, Arch::X64)),
        (Arch::Arm64, installer_name(version, Arch::Arm64)),
    ]
}

fn validity(days: u64) -> anyhow::Result<u64> {
    let seconds = days.saturating_mul(DAY);
    if days == 0 || seconds > MAX_VALIDITY_SECS {
        bail!(
            "--expires-days {days} is outside 1..={} (design m5b A.2)",
            MAX_VALIDITY_SECS / DAY
        );
    }
    Ok(seconds)
}

/// The production `prepare-release` (design m5b B.3 steps 1–14).
pub fn prepare_release(env: &mut Env<'_>, options: &Prepare) -> anyhow::Result<()> {
    // 1. The tag.
    let version = release_tag_version(&options.tag)?;
    let tag = options.tag.as_str();
    if options.commit.len() != 40
        || !options
            .commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("--commit must be the 40 lower-case hex digits of the reviewed commit");
    }
    if options.alt_key_id == Some(options.main_key_id) {
        bail!("--alt-key-id is the same key as --main-key-id");
    }
    let validity = validity(options.expires_days)?;
    if let Some(min_from) = &options.min_from_version {
        parse_release_version(min_from)
            .map_err(|_| anyhow::anyhow!("--min-from-version {min_from:?} is not X.Y.Z"))?;
    }
    if options.allow_strand.iter().any(|allowed| allowed == tag) {
        bail!("--allow-strand cannot name the release being prepared");
    }

    // 2. The local work tree.
    let head = env.repo.head()?;
    if head != options.commit {
        bail!("HEAD is {head}, not the reviewed commit {}", options.commit);
    }
    match env.repo.tag_commit(tag)? {
        Some(commit) if commit == options.commit => {}
        Some(commit) => bail!("the local tag {tag} is {commit}, not {}", options.commit),
        None => bail!("there is no local tag {tag} (git fetch --tags)"),
    }
    if !env.repo.is_clean()? {
        bail!("the work tree has changes: check out the tag cleanly");
    }

    // 3. The tag on GitHub.
    let remote = env.host.tag_commit(tag)?;
    if remote != options.commit {
        bail!(
            "the tag {tag} on GitHub is {remote}, not {}",
            options.commit
        );
    }

    // 4. The draft and its three assets.
    let view = env.host.release(tag)?;
    if !view.is_draft || view.is_prerelease {
        bail!("the release {tag} must be a draft and not a pre-release");
    }
    let names = installer_names(&version);
    let expected: BTreeSet<String> = names
        .iter()
        .map(|(_, name)| name.clone())
        .chain(["SHA256SUMS".to_string()])
        .collect();
    let actual: Vec<String> = view.assets.iter().map(|asset| asset.name.clone()).collect();
    if actual.len() != expected.len() || actual.iter().cloned().collect::<BTreeSet<_>>() != expected
    {
        bail!("the draft's assets are {actual:?}, expected exactly {expected:?}");
    }
    let assets_dir = options.out.join("assets");
    std::fs::create_dir_all(&assets_dir)
        .with_context(|| format!("creating {}", assets_dir.display()))?;
    let sums_path = env.host.download(tag, "SHA256SUMS", &assets_dir)?;
    let sums = sums::parse(&read_text(&sums_path)?)?;

    // 5. Hashes: the file, SHA256SUMS and GitHub's digest agree.
    if sums.len() != 2 {
        bail!(
            "SHA256SUMS has {} lines, expected the 2 installers",
            sums.len()
        );
    }
    let mut assets = Vec::new();
    let mut prepared_assets = Vec::new();
    for (arch, name) in &names {
        let path = env.host.download(tag, name, &assets_dir)?;
        let (digest, size) = hash_file(&path)?;
        let listed = sums
            .get(name)
            .with_context(|| format!("SHA256SUMS has no line for {name}"))?;
        if *listed != digest {
            bail!(
                "{name}: SHA256SUMS says {}, the file is {}",
                listed.to_hex(),
                digest.to_hex()
            );
        }
        let info = view
            .assets
            .iter()
            .find(|asset| &asset.name == name)
            .with_context(|| format!("no asset {name}"))?;
        let github_digest = info
            .digest
            .as_deref()
            .with_context(|| format!("GitHub reports no digest for {name}"))?;
        if github_digest != format!("sha256:{}", digest.to_hex()) || info.size != size {
            bail!("{name}: GitHub's digest or size differs from the downloaded file");
        }
        // 6. Provenance.
        env.host
            .verify_attestation(&path, &options.commit, tag)
            .with_context(|| format!("{name}: the build provenance does not verify"))?;
        assets.push(ManifestAsset {
            arch: *arch,
            name: name.clone(),
            size,
            sha256: digest.to_hex(),
        });
        prepared_assets.push(json!({
            "name": name,
            "size": size,
            "sha256": digest.to_hex(),
            "digest": github_digest,
        }));
    }

    // 7. The trusted time.
    let trusted = env.host.server_time()?;
    let local = env.clock.now();
    if trusted.abs_diff(local) > MAX_CLOCK_SKEW {
        bail!(
            "the local clock ({}) and GitHub's ({}) differ by more than 5 minutes: fix the clock",
            format_utc(local),
            format_utc(trusted)
        );
    }
    let issued_at = trusted;
    let expires = issued_at + validity;

    // 8. The published manifest, verified with the anchors of its own release.
    let releases = env.host.releases()?;
    let this_anchors = anchors_at(env.repo, tag)?
        .with_context(|| format!("{tag} embeds no trust anchors (keys)"))?;
    let published_revoked = published_manifest(env, options, &releases, issued_at, tag)?;

    // 9. Revocations: this tag's file, the published manifest's, and --revoke.
    let revoked: BTreeSet<KeyId> = this_anchors
        .revoked_ids()
        .into_iter()
        .chain(published_revoked.iter().copied())
        .chain(options.revoke.iter().copied())
        .collect();
    if !published_revoked.iter().all(|id| revoked.contains(id)) {
        bail!("the published manifest's revocations are not all carried over");
    }
    // In the order of their text, as people read them.
    let mut revoked: Vec<KeyId> = revoked.into_iter().collect();
    revoked.sort_by_key(|id| id.to_text());

    // 10. The strand check over the window and this tag.
    let window = window_targets(&releases, issued_at);
    let mut rows = Vec::new();
    for target in &window {
        if target.tag == tag {
            continue;
        }
        match anchors_at(env.repo, &target.tag)? {
            Some(anchors) => rows.push(strand_row(
                &target.tag,
                &anchors,
                options.main_key_id,
                options.alt_key_id,
                &revoked,
            )),
            None => writeln!(
                env.out,
                "note: {} has no trust anchors (it cannot update); not checked",
                target.tag
            )?,
        }
    }
    rows.push(strand_row(
        tag,
        &this_anchors,
        options.main_key_id,
        options.alt_key_id,
        &revoked,
    ));
    print_strand_table(env.out, &rows, &window, &options.allow_strand)?;
    let stranded: Vec<&str> = rows
        .iter()
        .filter(|row| !row.accepted() && !options.allow_strand.contains(&row.tag))
        .map(|row| row.tag.as_str())
        .collect();
    if !stranded.is_empty() {
        bail!(
            "these releases would not accept the manifest: {stranded:?} (sign with a key they \
             know, or name them in --allow-strand)"
        );
    }

    // 11. latest.json and the trusted comment.
    let mut key_ids = vec![options.main_key_id.to_text()];
    key_ids.extend(options.alt_key_id.map(KeyId::to_text));
    let manifest = Manifest {
        schema: 1,
        product: mklm_update::PRODUCT.to_string(),
        channel: mklm_update::CHANNEL.to_string(),
        version: version.to_string(),
        issued_at,
        expires,
        key_ids,
        revoked_keys: revoked.iter().map(|id| id.to_text()).collect(),
        min_from_version: options.min_from_version.clone(),
        assets,
    };
    let trusted_comment =
        format!("{TRUSTED_COMMENT_PREFIX} version={version} issued_at={issued_at}");
    let out = &options.out;
    write(&out.join("latest.json"), manifest.to_canonical_json())?;
    write(
        &out.join("trusted-comment.txt"),
        format!("{trusted_comment}\n"),
    )?;

    // 12. The offline commands, one per signature.
    let mut sign = String::new();
    for (key, slot_file) in [
        (Some(options.main_key_id), "latest.json.minisig"),
        (options.alt_key_id, "latest.json.alt.minisig"),
    ] {
        let Some(key) = key else { continue };
        let role = role_for_signing(key, &this_anchors, &rows, env.repo)?;
        let (minisign, key_file) = signing_media(role);
        sign.push_str(&format!(
            "{} -S -s {} -m {} -x {} -t \"{trusted_comment}\"\n",
            shell_program(Path::new(minisign)),
            shell_arg(Path::new(key_file)),
            shell_arg(&out.join("latest.json")),
            shell_arg(&out.join(slot_file)),
        ));
    }
    write(&out.join("SIGN-OFFLINE.txt"), &sign)?;

    // 13. prepare.json, for publish.
    let prepared = json!({
        "tag": tag,
        "commit": options.commit,
        "version": version.to_string(),
        "issued_at": issued_at,
        "expires": expires,
        "main_key_id": options.main_key_id.to_text(),
        "alt_key_id": options.alt_key_id.map(KeyId::to_text),
        "revoked_keys": revoked.iter().map(|id| id.to_text()).collect::<Vec<_>>(),
        "assets": prepared_assets,
        "window": window.iter().map(|target| json!({
            "tag": target.tag,
            "published_at": target.published_at,
            "successor_published_at": target.successor_published_at,
        })).collect::<Vec<_>>(),
        "allow_strand": options.allow_strand,
        "strand": rows.iter().map(|row| json!({
            "tag": row.tag,
            "main": verdict(&row.main),
            "alt": row.alt.as_ref().map(verdict),
            "accepted": row.accepted(),
        })).collect::<Vec<_>>(),
    });
    write(
        &out.join("prepare.json"),
        serde_json::to_string_pretty(&prepared)? + "\n",
    )?;

    // 14. The summary.
    let o = &mut *env.out;
    writeln!(o)?;
    writeln!(o, "MKLM {version} is ready to be signed offline.")?;
    writeln!(o, "  ISSUED:   {}", format_utc(issued_at))?;
    writeln!(
        o,
        "  EXPIRES:  {} ({} days)",
        format_date(expires),
        validity / DAY
    )?;
    writeln!(
        o,
        "  main key: {} ({})",
        options.main_key_id,
        this_anchors
            .role_of(options.main_key_id)
            .map_or("not in this release's anchors", KeyRole::as_str)
    )?;
    if let Some(alt) = options.alt_key_id {
        writeln!(
            o,
            "  alt key:  {alt} ({})",
            this_anchors
                .role_of(alt)
                .map_or("not in this release's anchors", KeyRole::as_str)
        )?;
    }
    writeln!(o, "  revoked:  {:?}", manifest.revoked_keys)?;
    for asset in &manifest.assets {
        writeln!(
            o,
            "  {}  {} bytes  {}",
            asset.name, asset.size, asset.sha256
        )?;
    }
    writeln!(
        o,
        "Next: design m5b B.5 steps 7-10 with {}",
        shown(&out.join("SIGN-OFFLINE.txt"))
    )?;
    Ok(())
}

/// Step 8: the published manifest and its revocations. `NotFound` only while no earlier tag has
/// trust anchors (the first release with the updater).
fn published_manifest(
    env: &mut Env<'_>,
    options: &Prepare,
    releases: &[crate::releases::PublishedRelease],
    now: u64,
    this_tag: &str,
) -> anyhow::Result<Vec<KeyId>> {
    let endpoints = Endpoints::production();
    let limits = Limits::manifest();
    let cancel = AtomicBool::new(false);
    let fetched = match fetch_manifest(env.transport, &endpoints, &limits, &cancel) {
        Ok(fetched) => fetched,
        Err(FetchError::NotFound) => {
            let mut earlier_with_anchors = Vec::new();
            for tag in env.repo.tags()? {
                if tag != this_tag && anchors_at(env.repo, &tag)?.is_some() {
                    earlier_with_anchors.push(tag);
                }
            }
            if !earlier_with_anchors.is_empty() {
                bail!(
                    "no latest.json is published, although {earlier_with_anchors:?} embed trust \
                     anchors: see the release incident runbook (design m5b B.5)"
                );
            }
            writeln!(
                env.out,
                "No latest.json is published yet (the first release with the updater)."
            )?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error).context("fetching the published latest.json"),
    };
    // The release it belongs to: the tag of its first redirect, else the newest stable one.
    let previous = match &fetched.tag {
        Some(tag) => tag.clone(),
        None => window_targets(releases, now)
            .last()
            .map(|target| target.tag.clone())
            .context("the published manifest's release cannot be told")?,
    };
    let anchors = anchors_at(env.repo, &previous)?.with_context(|| {
        format!("{previous} embeds no trust anchors, yet a manifest is published")
    })?;
    let alt = match fetch_alt_signature(
        env.transport,
        &endpoints,
        fetched.tag.as_deref(),
        &limits,
        &cancel,
    ) {
        Ok(alt) => alt,
        Err(error) => return Err(error).context("fetching the published alternate signature"),
    };
    let (verified, _) = verify_with_alternate(
        &fetched.manifest,
        Signatures {
            main: &fetched.signature,
            alt: alt.as_deref(),
        },
        &anchors,
        &Version::new(0, 0, 0),
        Arch::X64,
        now,
        fetched.tag.as_deref(),
    )
    .map_err(|refusal| {
        anyhow::anyhow!(
            "the published latest.json does not verify with the anchors of {previous}: {refusal}"
        )
    })?;
    writeln!(
        env.out,
        "Published: {} issued {} (signed by {}).",
        verified.version,
        format_utc(verified.issued_at),
        verified.signer
    )?;
    if verified.issued_at > now {
        if !options.published_misdated {
            bail!(
                "the published manifest is dated in the future ({}); if that is a known mistake, \
                 add --published-misdated (design m5b B.7)",
                format_utc(verified.issued_at)
            );
        }
    } else if now <= verified.issued_at {
        bail!(
            "issued_at {now} is not after the published manifest's {}",
            verified.issued_at
        );
    }
    let manifest =
        Manifest::parse(&fetched.manifest).map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    manifest
        .revoked_keys
        .iter()
        .map(|text| KeyId::parse(text).map_err(|error| anyhow::anyhow!("{error}")))
        .collect()
}

/// The role that decides which medium signs: this release's anchors first, else a release in
/// the window that knows the key.
fn role_for_signing(
    key: KeyId,
    this_anchors: &TrustAnchors,
    rows: &[StrandRow],
    repo: &mut dyn crate::host::Repo,
) -> anyhow::Result<KeyRole> {
    if let Some(role) = this_anchors.role_of(key) {
        return Ok(role);
    }
    for row in rows {
        if let Some(anchors) = anchors_at(repo, &row.tag)?
            && let Some(role) = anchors.role_of(key)
        {
            return Ok(role);
        }
    }
    bail!("no release in the window knows key {key}")
}

fn print_strand_table(
    out: &mut dyn Write,
    rows: &[StrandRow],
    window: &[crate::releases::WindowTarget],
    allowed: &[String],
) -> anyhow::Result<()> {
    writeln!(out, "Strand check (design m5b B.3 step 10):")?;
    for row in rows {
        let target = window.iter().find(|target| target.tag == row.tag);
        let successor = target
            .and_then(|target| target.successor_published_at.zip(target.window_ends()))
            .map(|(successor, ends)| {
                format!(
                    ", successor published {}, window ends {}",
                    format_date(successor),
                    format_date(ends)
                )
            })
            .unwrap_or_default();
        let alt = row
            .alt
            .as_ref()
            .map(|alt| format!(", alt: {}", verdict(alt)))
            .unwrap_or_default();
        let mark = if row.accepted() {
            "accepts"
        } else if allowed.contains(&row.tag) {
            "STRANDED (allowed)"
        } else {
            "STRANDED"
        };
        writeln!(
            out,
            "  {}: main: {}{alt} -> {mark}{successor}",
            row.tag,
            verdict(&row.main)
        )?;
    }
    Ok(())
}

/// The options of the rehearsal grammar (design m5b B.3 "prepare-release --dev").
#[derive(Debug, Clone)]
pub struct PrepareDev {
    pub tag: String,
    pub dist: PathBuf,
    pub dev_pub: PathBuf,
    pub minisign: PathBuf,
    pub only_arch: Option<Arch>,
    pub expires_days: u64,
    pub issued_at: Option<u64>,
    pub out: PathBuf,
}

/// `prepare-release --dev` (design m5b B.3, F.6): local files and the local clock only.
pub fn prepare_release_dev(
    options: &PrepareDev,
    release_anchors: Result<TrustAnchors, KeyError>,
    now: u64,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let version = release_tag_version(&options.tag)?;
    let validity = validity(options.expires_days)?;
    let (_, dev_id, _) = pubkey_line(&read_text(&options.dev_pub)?, KeyRole::Primary)
        .context("--dev-pub is not a minisign public key file")?;
    match release_anchors {
        Ok(anchors) if anchors.fingerprint_of(dev_id).is_some() => {
            bail!("--dev-pub is a production key ({dev_id}): never use it for a rehearsal");
        }
        Ok(_) | Err(KeyError::NotConfigured) => {}
        Err(error) => return Err(error).context("the committed trust anchors file"),
    }

    let sums = sums::parse(&read_text(&options.dist.join("SHA256SUMS"))?)?;
    let mut assets = Vec::new();
    let mut used = BTreeSet::new();
    for (arch, name) in installer_names(&version) {
        if options.only_arch.is_some_and(|only| only != arch) {
            // The other architecture, filled in (OPS-UX-TEST-9).
            assets.push(ManifestAsset {
                arch,
                name,
                size: 1,
                sha256: "0".repeat(64),
            });
            continue;
        }
        let (digest, size) = hash_file(&options.dist.join(&name))?;
        let listed = sums
            .get(&name)
            .with_context(|| format!("SHA256SUMS has no line for {name}"))?;
        if *listed != digest {
            bail!("{name}: SHA256SUMS does not match the file");
        }
        used.insert(name.clone());
        assets.push(ManifestAsset {
            arch,
            name,
            size,
            sha256: digest.to_hex(),
        });
    }
    let listed: BTreeSet<String> = sums.keys().cloned().collect();
    if listed != used {
        bail!("SHA256SUMS lists {listed:?}, the rehearsal uses {used:?}");
    }

    let issued_at = options.issued_at.unwrap_or(now);
    let expires = issued_at + validity;
    let manifest = Manifest {
        schema: 1,
        product: mklm_update::PRODUCT.to_string(),
        channel: mklm_update::CHANNEL.to_string(),
        version: version.to_string(),
        issued_at,
        expires,
        key_ids: vec![dev_id.to_text()],
        revoked_keys: Vec::new(),
        min_from_version: None,
        assets,
    };
    let trusted_comment =
        format!("{DEV_TRUSTED_COMMENT_PREFIX} version={version} issued_at={issued_at}");
    std::fs::create_dir_all(&options.out)?;
    write(
        &options.out.join("latest.json"),
        manifest.to_canonical_json(),
    )?;
    write(
        &options.out.join("trusted-comment.txt"),
        format!("{trusted_comment}\n"),
    )?;
    // The key file next to the .pub, same name: written as text, never opened.
    let key_file = options.dev_pub.with_extension("key");
    let sign = format!(
        "{} -S -s {} -m {} -x {} -t \"{trusted_comment}\"\n",
        shell_program(&options.minisign),
        shell_arg(&key_file),
        shell_arg(&options.out.join("latest.json")),
        shell_arg(&options.out.join("latest.json.minisig")),
    );
    write(&options.out.join("SIGN-OFFLINE.txt"), &sign)?;
    writeln!(
        out,
        "Rehearsal manifest of {version} (development key {dev_id}):"
    )?;
    writeln!(
        out,
        "  issued {}, expires {}",
        format_utc(issued_at),
        format_date(expires)
    )?;
    writeln!(out, "Sign it with:")?;
    write!(out, "  {sign}")?;
    Ok(())
}

/// The default of `--expires-days` (user decision J-3).
pub fn default_expires_days() -> u64 {
    DEFAULT_VALIDITY_DAYS
}
