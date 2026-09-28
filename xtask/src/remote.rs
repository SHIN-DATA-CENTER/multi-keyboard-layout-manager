//! `verify --remote` / `verify --dir` and `fetch-smoke` (design m5b B.3, F.8): the product's own
//! fetch path (`WinHttpTransport`, the production URL policy, the tag of the first redirect, the
//! alternate-signature rule) and verification, with the trust anchors of this tree and an empty
//! record.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, bail};
use mklm_update::fetch::{
    FetchError, Limits, download_asset, fetch_alt_signature, fetch_latest_file, fetch_manifest,
};
use mklm_update::url::Endpoints;
use mklm_update::{
    Arch, SignatureSlot, TrustAnchors, VerifiedManifest, Version, installer_name, tries_alternate,
};

use crate::common::{Env, Signatures, read, verify_with_alternate};
use crate::releases::stable_tag_version;
use crate::sums;
use crate::time::{DAY, format_date, format_utc};

/// What `verify --remote` checks besides the signature.
#[derive(Debug, Clone, Default)]
pub struct RemoteChecks {
    /// Also download both installers and check their sizes and SHA-256 (SECURITY-1 (6)).
    pub installers: bool,
    /// Fail when fewer days are left before `expires` (the canary: 60).
    pub min_days_left: Option<u64>,
    /// Fail when the served version is older than the newest published stable release
    /// (RED-TEAM-1).
    pub newest_published: bool,
    /// Fail unless the served version is this one (publish's check after publishing).
    pub expect_version: Option<Version>,
}

/// The served manifest, verified.
#[derive(Debug, Clone)]
pub struct Served {
    pub verified: VerifiedManifest,
    pub slot: SignatureSlot,
    pub tag: Option<String>,
}

/// `verify --remote` (design m5b B.3, G.6).
pub fn verify_remote(
    env: &mut Env<'_>,
    anchors: &TrustAnchors,
    checks: &RemoteChecks,
) -> anyhow::Result<Served> {
    let endpoints = Endpoints::production();
    let limits = Limits::manifest();
    let cancel = AtomicBool::new(false);
    let now = env.host.server_time()?;
    let fetched = fetch_manifest(env.transport, &endpoints, &limits, &cancel)
        .context("fetching latest.json and its signature")?;
    let installed = Version::new(0, 0, 0);
    let verify = |alt: Option<&[u8]>, arch: Arch| {
        verify_with_alternate(
            &fetched.manifest,
            Signatures {
                main: &fetched.signature,
                alt,
            },
            anchors,
            &installed,
            arch,
            now,
            fetched.tag.as_deref(),
        )
    };
    let mut alt = None;
    let (verified, slot) = match verify(None, Arch::X64) {
        Ok(result) => result,
        Err(error) if tries_alternate(&error) => {
            alt = fetch_alt_signature(
                env.transport,
                &endpoints,
                fetched.tag.as_deref(),
                &limits,
                &cancel,
            )
            .context("fetching the alternate signature")?;
            verify(alt.as_deref(), Arch::X64)
                .map_err(|refusal| anyhow::anyhow!("latest.json does not verify: {refusal}"))?
        }
        Err(refusal) => bail!("latest.json does not verify: {refusal}"),
    };
    let o = &mut *env.out;
    writeln!(
        o,
        "latest.json: MKLM {} (tag {}), signed by {} ({:?} signature)",
        verified.version,
        fetched.tag.as_deref().unwrap_or("unknown"),
        verified.signer,
        slot
    )?;
    writeln!(o, "  issued  {}", format_utc(verified.issued_at))?;
    writeln!(o, "  expires {}", format_utc(verified.expires))?;
    if verified.issued_at > now + DAY {
        bail!(
            "issued_at ({}) is more than a day after GitHub's time ({})",
            format_utc(verified.issued_at),
            format_utc(now)
        );
    }
    if let Some(days) = checks.min_days_left {
        let left = verified.expires.saturating_sub(now) / DAY;
        writeln!(o, "  {left} days left before it expires")?;
        if verified.expires < now + days * DAY {
            bail!(
                "latest.json expires on {} ({left} days left, fewer than {days}): publish a \
                 maintenance release (design m5b B.4, B.5)",
                format_date(verified.expires)
            );
        }
    }
    if let Some(expected) = &checks.expect_version
        && verified.version != *expected
    {
        bail!("the served version is {}, not {expected}", verified.version);
    }
    if checks.newest_published {
        let newest = env
            .host
            .releases()?
            .iter()
            .filter_map(|release| stable_tag_version(&release.tag))
            .max_by(|a, b| a.cmp_precedence(b));
        if let Some(newest) = newest
            && newest.cmp_precedence(&verified.version).is_gt()
        {
            bail!(
                "the served latest.json is {}, but {newest} is published: the latest mark was \
                 removed or a stale manifest is served (design m5b B.4, RED-TEAM-1)",
                verified.version
            );
        }
    }
    if checks.installers {
        for arch in [Arch::X64, Arch::Arm64] {
            let (for_arch, _) = verify(alt.as_deref(), arch)
                .map_err(|refusal| anyhow::anyhow!("latest.json does not verify: {refusal}"))?;
            let asset = for_arch.asset;
            download_asset(
                env.transport,
                &endpoints,
                &for_arch.version,
                &asset,
                &Limits::installer(),
                &mut std::io::sink(),
                &mut |_| {},
                &cancel,
            )
            .with_context(|| format!("downloading {}", asset.name))?;
            writeln!(
                env.out,
                "  {}: {} bytes, SHA-256 {} as signed",
                asset.name,
                asset.size,
                asset.sha256.to_hex()
            )?;
        }
    }
    Ok(Served {
        verified,
        slot,
        tag: fetched.tag,
    })
}

/// `verify --dir`: local `latest.json`, `latest.json.minisig` and, if present,
/// `latest.json.alt.minisig`.
pub fn verify_dir(
    dir: &Path,
    anchors: &TrustAnchors,
    now: u64,
    out: &mut dyn Write,
) -> anyhow::Result<VerifiedManifest> {
    let manifest = read(&dir.join(mklm_update::MANIFEST_NAME))?;
    let main = read(&dir.join(mklm_update::SIGNATURE_NAME))?;
    let alt_path = dir.join(mklm_update::ALT_SIGNATURE_NAME);
    let alt = if alt_path.is_file() {
        Some(read(&alt_path)?)
    } else {
        None
    };
    let (verified, slot) = verify_with_alternate(
        &manifest,
        Signatures {
            main: &main,
            alt: alt.as_deref(),
        },
        anchors,
        &Version::new(0, 0, 0),
        Arch::X64,
        now,
        None,
    )
    .map_err(|refusal| anyhow::anyhow!("{} does not verify: {refusal}", dir.display()))?;
    writeln!(
        out,
        "{}: MKLM {} signed by {} ({slot:?} signature), issued {}, expires {} ({:?})",
        dir.display(),
        verified.version,
        verified.signer,
        format_utc(verified.issued_at),
        format_date(verified.expires),
        verified.freshness
    )?;
    Ok(verified)
}

/// `fetch-smoke` (design m5b F.8; OPS-UX-TEST-8): `releases/latest/download/SHA256SUMS` over
/// the production path; every redirect passes the production rules, the tag comes from the first
/// one, and the body is the SHA256SUMS of that release's two installers.
pub fn fetch_smoke(
    transport: &mut dyn mklm_update::fetch::Transport,
    out: &mut dyn Write,
) -> anyhow::Result<String> {
    let cancel = AtomicBool::new(false);
    let (body, tag) = match fetch_latest_file(
        transport,
        &Endpoints::production(),
        "SHA256SUMS",
        64 * 1024,
        &Limits::manifest(),
        &cancel,
    ) {
        Ok(result) => result,
        Err(FetchError::RedirectNotAllowed { location }) => bail!(
            "GitHub redirected to {location:?}, which the production URL policy refuses: the \
             distribution changed (design m5b A.6, G.7 1)"
        ),
        Err(error) => return Err(error).context("fetching releases/latest/download/SHA256SUMS"),
    };
    let tag = tag.context(
        "the first redirect did not name a tag (releases/download/vX.Y.Z/SHA256SUMS): the \
         signatures would be fetched without a tag (design m5b A.6)",
    )?;
    let version =
        stable_tag_version(&tag).with_context(|| format!("{tag} is not a release tag"))?;
    let text = String::from_utf8(body).context("SHA256SUMS is not UTF-8")?;
    let listed = sums::parse(&text)?;
    for arch in [Arch::X64, Arch::Arm64] {
        let name = installer_name(&version, arch);
        if !listed.contains_key(&name) {
            bail!("SHA256SUMS of {tag} has no line for {name}");
        }
    }
    writeln!(
        out,
        "fetch-smoke: ok (tag {tag}, {} files listed)",
        listed.len()
    )?;
    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{DAY as TEST_DAY, FakeClock, FakeHost, FakeRepo, FakeWeb, Key, T0};
    use mklm_update::{KeyRole, Manifest, ManifestAsset, Sha256Digest};

    fn manifest(version: &str, issued_at: u64, key: &Key, installers: &[(Arch, &[u8])]) -> Vec<u8> {
        let parsed = Version::parse(version).unwrap();
        Manifest {
            schema: 1,
            product: "MKLM".to_string(),
            channel: "stable".to_string(),
            version: version.to_string(),
            issued_at,
            expires: issued_at + 180 * TEST_DAY,
            key_ids: vec![key.id.to_text()],
            revoked_keys: Vec::new(),
            min_from_version: None,
            assets: installers
                .iter()
                .map(|(arch, content)| ManifestAsset {
                    arch: *arch,
                    name: installer_name(&parsed, *arch),
                    size: content.len() as u64,
                    sha256: Sha256Digest::of(content).to_hex(),
                })
                .collect(),
        }
        .to_canonical_json()
        .into_bytes()
    }

    struct Setup {
        host: FakeHost,
        repo: FakeRepo,
        clock: FakeClock,
        web: FakeWeb,
        anchors: TrustAnchors,
    }

    fn setup(issued_at: u64) -> Setup {
        let (p, b) = (Key::new(), Key::new());
        let anchors = TrustAnchors::from_keys(
            &[(KeyRole::Primary, &p.public), (KeyRole::Backup, &b.public)],
            &[],
        )
        .unwrap();
        let x64: &[u8] = b"x64 installer";
        let arm64: &[u8] = b"arm64 installer";
        let latest = manifest(
            "0.2.1",
            issued_at,
            &p,
            &[(Arch::X64, x64), (Arch::Arm64, arm64)],
        );
        let signature = p.sign(
            &latest,
            &format!("mklm-latest-json v1 version=0.2.1 issued_at={issued_at}"),
        );
        let mut web = FakeWeb::default();
        web.latest_release(
            "v0.2.1",
            &[
                ("latest.json", &latest),
                ("latest.json.minisig", &signature),
            ],
        );
        web.file("/releases/download/v0.2.1/MKLM-Setup-0.2.1-x64.exe", x64);
        web.file(
            "/releases/download/v0.2.1/MKLM-Setup-0.2.1-arm64.exe",
            arm64,
        );
        let mut host = FakeHost {
            time: T0 + TEST_DAY,
            ..FakeHost::default()
        };
        host.published_release("v0.2.0", T0 - 30 * TEST_DAY);
        host.published_release("v0.2.1", T0);
        Setup {
            host,
            repo: FakeRepo::default(),
            clock: FakeClock::default(),
            web,
            anchors,
        }
    }

    fn run(setup: &mut Setup, checks: &RemoteChecks) -> anyhow::Result<Served> {
        let mut out = Vec::new();
        let mut env = Env {
            host: &mut setup.host,
            repo: &mut setup.repo,
            clock: &mut setup.clock,
            transport: &mut setup.web,
            out: &mut out,
        };
        let anchors = setup.anchors.clone();
        verify_remote(&mut env, &anchors, checks)
    }

    #[test]
    fn the_canary_passes_a_good_release() {
        let mut setup = setup(T0);
        let served = run(
            &mut setup,
            &RemoteChecks {
                installers: true,
                min_days_left: Some(60),
                newest_published: true,
                expect_version: Some(Version::new(0, 2, 1)),
            },
        )
        .unwrap();
        assert_eq!(served.verified.version, Version::new(0, 2, 1));
        assert_eq!(served.tag.as_deref(), Some("v0.2.1"));
        assert_eq!(served.slot, SignatureSlot::Main);
        assert!(
            setup
                .web
                .requests
                .iter()
                .any(|url| url.ends_with("MKLM-Setup-0.2.1-arm64.exe"))
        );
    }

    #[test]
    fn the_canary_fails() {
        // Fewer than 60 days left (issued 121 days ago).
        let mut setup = setup(T0 - 120 * TEST_DAY);
        let checks = RemoteChecks {
            min_days_left: Some(60),
            ..RemoteChecks::default()
        };
        assert!(run(&mut setup, &checks).is_err());
        // Issued more than a day in the future.
        let mut setup = setup_future();
        assert!(run(&mut setup, &RemoteChecks::default()).is_err());
        // A newer stable release is published (the latest mark was removed).
        let mut setup = self::setup(T0);
        setup.host.published_release("v0.2.2", T0 + TEST_DAY / 2);
        let checks = RemoteChecks {
            newest_published: true,
            ..RemoteChecks::default()
        };
        assert!(run(&mut setup, &checks).is_err());
        // A pre-release is not newer.
        let mut setup = self::setup(T0);
        setup
            .host
            .published_release("v0.3.0-rc.1", T0 + TEST_DAY / 2);
        assert!(run(&mut setup, &checks).is_ok());
        // An installer that differs from the manifest.
        let mut setup = self::setup(T0);
        setup.web.file(
            "/releases/download/v0.2.1/MKLM-Setup-0.2.1-arm64.exe",
            b"arm64 installeR",
        );
        let checks = RemoteChecks {
            installers: true,
            ..RemoteChecks::default()
        };
        assert!(run(&mut setup, &checks).is_err());
        // No manifest at all.
        let mut setup = self::setup(T0);
        setup.web.routes.clear();
        assert!(run(&mut setup, &RemoteChecks::default()).is_err());
        // Signed by a key this tree does not know.
        let mut setup = self::setup(T0);
        setup.anchors =
            TrustAnchors::from_keys(&[(KeyRole::Primary, &Key::new().public)], &[]).unwrap();
        assert!(run(&mut setup, &RemoteChecks::default()).is_err());
    }

    fn setup_future() -> Setup {
        let mut setup = setup(T0 + 3 * TEST_DAY);
        setup.host.time = T0 + TEST_DAY;
        setup
    }

    #[test]
    fn fetch_smoke_of_v010() {
        let sums = format!(
            "{}  MKLM-Setup-0.1.0-arm64.exe\n{}  MKLM-Setup-0.1.0-x64.exe\n",
            "a".repeat(64),
            "b".repeat(64)
        );
        let mut web = FakeWeb::default();
        web.latest_release("v0.1.0", &[("SHA256SUMS", sums.as_bytes())]);
        let mut out = Vec::new();
        assert_eq!(fetch_smoke(&mut web, &mut out).unwrap(), "v0.1.0");
        // A redirect to an unknown host, a missing tag, a body that is not SHA256SUMS.
        let mut web = FakeWeb::default();
        web.routes.insert(
            format!(
                "{}/releases/latest/download/SHA256SUMS",
                mklm_update::REPO_URL
            ),
            (
                302,
                vec![("Location".to_string(), "https://example.com/x".to_string())],
                Vec::new(),
            ),
        );
        assert!(fetch_smoke(&mut web, &mut out).is_err());
        let mut web = FakeWeb::default();
        web.file("/releases/latest/download/SHA256SUMS", sums.as_bytes());
        assert!(fetch_smoke(&mut web, &mut out).is_err());
        let mut web = FakeWeb::default();
        web.latest_release("v0.1.0", &[("SHA256SUMS", b"<html>")]);
        assert!(fetch_smoke(&mut web, &mut out).is_err());
        let mut web = FakeWeb::default();
        web.latest_release(
            "v0.1.0",
            &[(
                "SHA256SUMS",
                format!("{}  other.exe\n", "a".repeat(64)).as_bytes(),
            )],
        );
        assert!(fetch_smoke(&mut web, &mut out).is_err());
    }
}
