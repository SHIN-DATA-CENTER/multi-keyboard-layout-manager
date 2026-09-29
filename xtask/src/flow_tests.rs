//! `prepare-release`, `prepare-release --dev` and `publish` against a fake GitHub, a fake
//! repository and a fake web with throwaway keys (design m5b F.1, the xtask tests). No network.

use std::path::Path;

use mklm_update::keys::KeyError;
use mklm_update::{Arch, KeyId, KeyRole, Manifest, TrustAnchors, Version, installer_name};
use serde_json::Value;

use crate::common::{Env, shell_arg};
use crate::prepare::{Prepare, PrepareDev, prepare_release, prepare_release_dev};
use crate::publish::publish;
#[cfg(windows)]
use crate::testing::powershell_command_elements;
use crate::testing::{DAY, FakeClock, FakeHost, FakeRepo, FakeWeb, Key, T0, TempDir, anchors_text};

/// The commit of the release being prepared.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

use KeyRole::{Backup, Primary};

/// A published release: its manifest and main signature as the fake web serves them.
fn published_manifest(
    version: &str,
    issued_at: u64,
    signer: &Key,
    key_ids: &[&Key],
    revoked: &[KeyId],
) -> (Vec<u8>, Vec<u8>) {
    let parsed = Version::parse(version).unwrap();
    let manifest = Manifest {
        schema: 1,
        product: "MKLM".to_string(),
        channel: "stable".to_string(),
        version: version.to_string(),
        issued_at,
        expires: issued_at + 180 * DAY,
        key_ids: key_ids.iter().map(|key| key.id.to_text()).collect(),
        revoked_keys: revoked.iter().map(|id| id.to_text()).collect(),
        min_from_version: None,
        assets: [Arch::X64, Arch::Arm64]
            .into_iter()
            .map(|arch| mklm_update::ManifestAsset {
                arch,
                name: installer_name(&parsed, arch),
                size: 10,
                sha256: "ab".repeat(32),
            })
            .collect(),
    }
    .to_canonical_json()
    .into_bytes();
    let signature = signer.sign(
        &manifest,
        &format!("mklm-latest-json v1 version={version} issued_at={issued_at}"),
    );
    (manifest, signature)
}

/// The installers and SHA256SUMS of a draft.
fn draft_assets(version: &str) -> Vec<(String, Vec<u8>)> {
    let parsed = Version::parse(version).unwrap();
    let x64 = format!("x64 installer {version}").into_bytes();
    let arm64 = format!("arm64 installer {version}").into_bytes();
    let sums = format!(
        "{}  {}\n{}  {}\n",
        mklm_update::Sha256Digest::of(&arm64).to_hex(),
        installer_name(&parsed, Arch::Arm64),
        mklm_update::Sha256Digest::of(&x64).to_hex(),
        installer_name(&parsed, Arch::X64),
    );
    vec![
        (installer_name(&parsed, Arch::X64), x64),
        (installer_name(&parsed, Arch::Arm64), arm64),
        ("SHA256SUMS".to_string(), sums.into_bytes()),
    ]
}

/// v0.1.0 (no updater), v0.2.0 (published, P1/B1) and the draft v0.2.1 (P1/B1).
struct World {
    host: FakeHost,
    repo: FakeRepo,
    clock: FakeClock,
    web: FakeWeb,
    p1: Key,
    b1: Key,
    out: TempDir,
}

/// The time of preparing: 10 days after v0.2.0.
const NOW: u64 = T0 + 10 * DAY;

fn world() -> World {
    let (p1, b1) = (Key::new(), Key::new());
    let anchors = anchors_text(&[(Primary, &p1), (Backup, &b1)], &[]);
    let mut repo = FakeRepo {
        head: COMMIT.to_string(),
        ..FakeRepo::default()
    };
    repo.tag("v0.1.0", &"1".repeat(40), None);
    repo.tag("v0.2.0", &"2".repeat(40), Some(anchors.clone()));
    repo.tag("v0.2.1", COMMIT, Some(anchors));
    let mut host = FakeHost {
        time: NOW,
        ..FakeHost::default()
    };
    host.published_release("v0.1.0", T0 - 400 * DAY);
    host.published_release("v0.2.0", T0);
    let assets = draft_assets("0.2.1");
    let assets: Vec<(&str, Vec<u8>)> = assets
        .iter()
        .map(|(name, content)| (name.as_str(), content.clone()))
        .collect();
    host.draft("v0.2.1", COMMIT, &assets);
    let mut web = FakeWeb::default();
    let (manifest, signature) = published_manifest("0.2.0", T0, &p1, &[&p1], &[]);
    web.latest_release(
        "v0.2.0",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    World {
        host,
        repo,
        clock: FakeClock {
            now: NOW + 30,
            ..FakeClock::default()
        },
        web,
        p1,
        b1,
        out: TempDir::new("prepare"),
    }
}

fn options(world: &World, main: &Key) -> Prepare {
    Prepare {
        tag: "v0.2.1".to_string(),
        commit: COMMIT.to_string(),
        main_key_id: main.id,
        alt_key_id: None,
        expires_days: 180,
        revoke: Vec::new(),
        min_from_version: None,
        allow_strand: Vec::new(),
        published_misdated: false,
        out: world.out.path().to_path_buf(),
    }
}

fn prepare(world: &mut World, options: &Prepare) -> anyhow::Result<String> {
    let mut printed = Vec::new();
    let mut env = Env {
        host: &mut world.host,
        repo: &mut world.repo,
        clock: &mut world.clock,
        transport: &mut world.web,
        out: &mut printed,
    };
    let result = prepare_release(&mut env, options);
    let printed = String::from_utf8(printed).unwrap();
    result.map(|()| printed)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_release_is_prepared() {
    let mut world = world();
    let options = options(&world, &world.p1);
    let printed = prepare(&mut world, &options).unwrap();
    let out = world.out.path();
    let manifest = Manifest::parse(&std::fs::read(out.join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest.version, "0.2.1");
    assert_eq!(manifest.issued_at, NOW);
    // J-3: 180 days by default.
    assert_eq!(manifest.expires, NOW + 180 * DAY);
    assert_eq!(manifest.key_ids, vec![world.p1.id.to_text()]);
    assert!(manifest.revoked_keys.is_empty());
    assert_eq!(manifest.assets.len(), 2);
    assert_eq!(
        std::fs::read_to_string(out.join("latest.json")).unwrap(),
        manifest.to_canonical_json()
    );
    let comment = std::fs::read_to_string(out.join("trusted-comment.txt")).unwrap();
    assert_eq!(
        comment,
        format!("mklm-latest-json v1 version=0.2.1 issued_at={NOW}\n")
    );
    let sign = std::fs::read_to_string(out.join("SIGN-OFFLINE.txt")).unwrap();
    assert_eq!(sign.lines().count(), 1);
    assert!(
        sign.starts_with(r"E:\tools\minisign.exe -S -s E:\mklm-keys\mklm-primary.key -m "),
        "{sign}"
    );
    assert!(
        sign.contains(&format!(
            "-m {} -x {} -t",
            shell_arg(&out.join("latest.json")),
            shell_arg(&out.join("latest.json.minisig"))
        )),
        "{sign}"
    );
    assert!(
        sign.trim_end()
            .ends_with(&format!("-t \"{}\"", comment.trim_end())),
        "{sign}"
    );
    let prepared = read_json(&out.join("prepare.json"));
    assert_eq!(prepared["tag"], "v0.2.1");
    assert_eq!(prepared["commit"], COMMIT);
    assert_eq!(prepared["issued_at"], NOW);
    assert_eq!(prepared["assets"].as_array().unwrap().len(), 2);
    let window: Vec<&str> = prepared["window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["tag"].as_str().unwrap())
        .collect();
    // v0.1.0 is in the window too (v0.2.0 came out 10 days ago), but it cannot update.
    assert_eq!(window, ["v0.1.0", "v0.2.0"]);
    assert!(
        printed.contains("note: v0.1.0 has no trust anchors"),
        "{printed}"
    );
    assert!(printed.contains("EXPIRES:  "), "{printed}");
    assert!(printed.contains("v0.2.0: main: ok"), "{printed}");
    // The downloaded assets are kept for the maintainer.
    assert!(out.join("assets").join("SHA256SUMS").is_file());
    // Nothing touched GitHub's state.
    assert!(world.host.uploads.is_empty() && world.host.publishes.is_empty());
}

#[test]
fn expiry_days_up_to_800() {
    let mut world = world();
    let mut options = options(&world, &world.p1);
    options.expires_days = 800;
    prepare(&mut world, &options).unwrap();
    let manifest =
        Manifest::parse(&std::fs::read(world.out.path().join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest.expires, NOW + 800 * DAY);
    options.expires_days = 801;
    assert!(prepare(&mut world, &options).is_err());
    options.expires_days = 0;
    assert!(prepare(&mut world, &options).is_err());
}

#[test]
fn every_check_before_signing() {
    type Change = Box<dyn Fn(&mut World, &mut Prepare)>;
    // (case, a part of the expected error, the change).
    let cases: Vec<(&str, &str, Change)> = vec![
        (
            "a pre-release tag",
            "pre-release tag",
            Box::new(|_, o| o.tag = "v0.2.1-rc.1".to_string()),
        ),
        (
            "HEAD is another commit",
            "HEAD is",
            Box::new(|w, _| w.repo.head = "f".repeat(40)),
        ),
        (
            "the local tag is another commit",
            "the local tag",
            Box::new(|w, _| {
                w.repo.tag(
                    "v0.2.1",
                    &"e".repeat(40),
                    Some(anchors_text(&[(Primary, &w.p1), (Backup, &w.b1)], &[])),
                );
            }),
        ),
        (
            "uncommitted changes",
            "changes",
            Box::new(|w, _| w.repo.dirty = true),
        ),
        (
            "GitHub's tag is another commit",
            "on GitHub",
            Box::new(|w, _| {
                w.host
                    .tag_commits
                    .insert("v0.2.1".to_string(), "d".repeat(40));
            }),
        ),
        (
            "not a draft",
            "must be a draft",
            Box::new(|w, _| w.host.views.get_mut("v0.2.1").unwrap().is_draft = false),
        ),
        (
            "a pre-release draft",
            "must be a draft",
            Box::new(|w, _| w.host.views.get_mut("v0.2.1").unwrap().is_prerelease = true),
        ),
        (
            "an extra asset",
            "the draft's assets",
            Box::new(|w, _| {
                let view = w.host.views.get_mut("v0.2.1").unwrap();
                let mut extra = view.assets[0].clone();
                extra.name = "notes.txt".to_string();
                view.assets.push(extra);
            }),
        ),
        (
            "SHA256SUMS disagrees",
            "SHA256SUMS says",
            Box::new(|w, _| {
                let name = "MKLM-Setup-0.2.1-x64.exe".to_string();
                w.host.files.insert(
                    ("v0.2.1".to_string(), name),
                    b"another x64 installer".to_vec(),
                );
                let view = w.host.views.get_mut("v0.2.1").unwrap();
                view.assets[0].size = 21;
                view.assets[0].digest = Some(crate::testing::digest_text(b"another x64 installer"));
            }),
        ),
        (
            "GitHub's digest disagrees",
            "GitHub's digest",
            Box::new(|w, _| {
                w.host.views.get_mut("v0.2.1").unwrap().assets[1].digest =
                    Some(format!("sha256:{}", "0".repeat(64)));
            }),
        ),
        (
            "no digest",
            "no digest",
            Box::new(|w, _| w.host.views.get_mut("v0.2.1").unwrap().assets[1].digest = None),
        ),
        (
            "no provenance",
            "provenance",
            Box::new(|w, _| w.host.attestation_fails = true),
        ),
        (
            "the local clock is 6 minutes off",
            "5 minutes",
            Box::new(|w, _| w.clock.now = NOW + 6 * 60),
        ),
        (
            "issued_at not after the published one",
            "is not after",
            Box::new(|w, _| {
                w.host.time = T0;
                w.clock.now = T0;
            }),
        ),
        (
            "the published manifest does not verify",
            "does not verify",
            Box::new(|w, _| {
                let (manifest, _) = published_manifest("0.2.0", T0, &w.p1, &[&w.p1], &[]);
                let signature = Key::new().sign(&manifest, "mklm-latest-json v1");
                w.web.latest_release(
                    "v0.2.0",
                    &[
                        ("latest.json", &manifest),
                        ("latest.json.minisig", &signature),
                    ],
                );
            }),
        ),
        (
            "--revoke of the signing key",
            "would not accept",
            Box::new(|w, o| o.revoke = vec![w.p1.id]),
        ),
        (
            "a key v0.2.0 does not know",
            "would not accept",
            Box::new(|w, o| {
                let p2 = Key::new();
                w.repo.tag(
                    "v0.2.1",
                    COMMIT,
                    Some(anchors_text(&[(Primary, &p2), (Backup, &w.b1)], &[&w.p1])),
                );
                o.main_key_id = p2.id;
            }),
        ),
        (
            "no latest.json although v0.2.0 has anchors",
            "no latest.json is published",
            Box::new(|w, _| w.web.routes.clear()),
        ),
        (
            "a release of the window is not a local tag",
            "not in the local repository",
            Box::new(|w, _| {
                w.repo.tags.remove("v0.1.0");
            }),
        ),
        (
            "the alternate key is the main key",
            "same key",
            Box::new(|w, o| o.alt_key_id = Some(w.p1.id)),
        ),
        (
            "a short commit",
            "40 lower-case",
            Box::new(|_, o| o.commit = "0123".to_string()),
        ),
        (
            "a malformed --min-from-version",
            "--min-from-version",
            Box::new(|_, o| o.min_from_version = Some("v0.2".to_string())),
        ),
    ];
    for (name, expected, change) in &cases {
        let mut world = world();
        let mut options = options(&world, &world.p1);
        change(&mut world, &mut options);
        let error = format!("{:#}", prepare(&mut world, &options).unwrap_err());
        assert!(error.contains(expected), "{name}: {error}");
        assert!(
            !world.out.path().join("SIGN-OFFLINE.txt").exists(),
            "{name}: nothing to sign"
        );
    }
}

#[test]
fn a_misdated_published_manifest() {
    let mut world = world();
    // v0.2.0's manifest was dated 30 days into the future.
    let (manifest, signature) =
        published_manifest("0.2.0", NOW + 30 * DAY, &world.p1, &[&world.p1], &[]);
    world.web.latest_release(
        "v0.2.0",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    let mut options = options(&world, &world.p1);
    assert!(prepare(&mut world, &options).is_err());
    options.published_misdated = true;
    prepare(&mut world, &options).unwrap();
}

#[test]
fn the_first_release_with_the_updater_has_nothing_published() {
    let mut world = world();
    world.web.routes.clear();
    world.repo.tags.remove("v0.2.0");
    world
        .host
        .published
        .retain(|release| release.tag != "v0.2.0");
    let options = options(&world, &world.p1);
    let printed = prepare(&mut world, &options).unwrap();
    assert!(
        printed.contains("No latest.json is published yet"),
        "{printed}"
    );
}

#[test]
fn published_revocations_are_carried_over() {
    let mut world = world();
    let gone = KeyId::parse("1111222233334444").unwrap();
    let (manifest, signature) = published_manifest("0.2.0", T0, &world.p1, &[&world.p1], &[gone]);
    world.web.latest_release(
        "v0.2.0",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    let extra = KeyId::parse("5555666677778888").unwrap();
    let mut options = options(&world, &world.p1);
    options.revoke = vec![extra];
    prepare(&mut world, &options).unwrap();
    let manifest =
        Manifest::parse(&std::fs::read(world.out.path().join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest.revoked_keys, vec![gone.to_text(), extra.to_text()]);
}

/// The example of design m5b B.3 (FIX-VERIFICATION-2): v0.4.0 (P1, B1) was the latest for a year;
/// v0.5.0 moved to P2 (signed B1 + P2); 60 days later v0.6.0 with P2 alone would strand v0.4.0,
/// with the alternate B1 it does not.
#[test]
fn the_window_example_of_design_b3() {
    let (p1, b1, p2) = (Key::new(), Key::new(), Key::new());
    let old = anchors_text(&[(Primary, &p1), (Backup, &b1)], &[]);
    let new = anchors_text(&[(Primary, &p2), (Backup, &b1)], &[&p1]);
    let v040 = crate::time::parse_rfc3339_utc("2027-01-10T00:00:00Z").unwrap();
    let v050 = crate::time::parse_rfc3339_utc("2028-01-10T00:00:00Z").unwrap();
    let now = crate::time::parse_rfc3339_utc("2028-03-10T00:00:00Z").unwrap();
    let mut world = world();
    world.repo.tags.clear();
    world.repo.tag("v0.4.0", &"4".repeat(40), Some(old));
    world.repo.tag("v0.5.0", &"5".repeat(40), Some(new.clone()));
    world.repo.tag("v0.6.0", COMMIT, Some(new));
    world.host.published.clear();
    world.host.published_release("v0.4.0", v040);
    world.host.published_release("v0.5.0", v050);
    world.host.time = now;
    world.clock.now = now;
    let assets = draft_assets("0.6.0");
    let assets: Vec<(&str, Vec<u8>)> = assets
        .iter()
        .map(|(name, content)| (name.as_str(), content.clone()))
        .collect();
    world.host.draft("v0.6.0", COMMIT, &assets);
    let (manifest, signature) = published_manifest("0.5.0", v050, &b1, &[&b1, &p2], &[p1.id]);
    world.web.routes.clear();
    world.web.latest_release(
        "v0.5.0",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    let mut options = options(&world, &p2);
    options.tag = "v0.6.0".to_string();
    let error = prepare(&mut world, &options).unwrap_err();
    assert!(error.to_string().contains("v0.4.0"), "{error}");
    // With the alternate signature by B1, every release in the window accepts it.
    options.alt_key_id = Some(b1.id);
    let printed = prepare(&mut world, &options).unwrap();
    assert!(
        printed.contains("v0.4.0: main: no (unknown key), alt: ok"),
        "{printed}"
    );
    let sign = std::fs::read_to_string(world.out.path().join("SIGN-OFFLINE.txt")).unwrap();
    assert_eq!(sign.lines().count(), 2);
    assert!(
        sign.lines()
            .nth(1)
            .unwrap()
            .starts_with(r"F:\tools\minisign.exe -S -s F:\mklm-keys-backup\mklm-backup.key")
    );
    assert!(
        sign.lines()
            .nth(1)
            .unwrap()
            .contains("latest.json.alt.minisig")
    );
    // Naming v0.4.0 in --allow-strand lets P2 alone through.
    options.alt_key_id = None;
    options.allow_strand = vec!["v0.4.0".to_string()];
    let printed = prepare(&mut world, &options).unwrap();
    assert!(printed.contains("STRANDED (allowed)"), "{printed}");
}

/// FIX-VERIFICATION-1: the manifests of the backup key rotation are not stranded.
#[test]
fn the_backup_key_rotation_strands_nobody() {
    let (p1, b1, p2, b2) = (Key::new(), Key::new(), Key::new(), Key::new());
    // (a) B1 leaked: v0.5.0 embeds P1, B2, revoked B1; signed by P1 with B1 revoked.
    let mut world = world();
    world.repo.tags.clear();
    world.repo.tag(
        "v0.4.0",
        &"4".repeat(40),
        Some(anchors_text(&[(Primary, &p1), (Backup, &b1)], &[])),
    );
    world.repo.tag(
        "v0.5.0",
        COMMIT,
        Some(anchors_text(&[(Primary, &p1), (Backup, &b2)], &[&b1])),
    );
    world.host.published.clear();
    world.host.published_release("v0.4.0", T0);
    let assets = draft_assets("0.5.0");
    let assets: Vec<(&str, Vec<u8>)> = assets
        .iter()
        .map(|(name, content)| (name.as_str(), content.clone()))
        .collect();
    world.host.draft("v0.5.0", COMMIT, &assets);
    let (manifest, signature) = published_manifest("0.4.0", T0, &p1, &[&p1], &[]);
    world.web.routes.clear();
    world.web.latest_release(
        "v0.4.0",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    let mut options = options(&world, &p1);
    options.tag = "v0.5.0".to_string();
    options.revoke = vec![b1.id];
    prepare(&mut world, &options).unwrap();

    // (b) After the window of a P1 → P2 move: v0.7.0 embeds P2, B2, revoked P1 and B1; v0.6.0
    // (P2, B1, revoked P1) is in the window.
    let mut world = self::world();
    world.repo.tags.clear();
    world.repo.tag(
        "v0.6.0",
        &"6".repeat(40),
        Some(anchors_text(&[(Primary, &p2), (Backup, &b1)], &[&p1])),
    );
    world.repo.tag(
        "v0.7.0",
        COMMIT,
        Some(anchors_text(&[(Primary, &p2), (Backup, &b2)], &[&p1, &b1])),
    );
    world.host.published.clear();
    world.host.published_release("v0.6.0", T0);
    let assets = draft_assets("0.7.0");
    let assets: Vec<(&str, Vec<u8>)> = assets
        .iter()
        .map(|(name, content)| (name.as_str(), content.clone()))
        .collect();
    world.host.draft("v0.7.0", COMMIT, &assets);
    let (manifest, signature) = published_manifest("0.6.0", T0, &p2, &[&p2], &[p1.id]);
    world.web.routes.clear();
    world.web.latest_release(
        "v0.6.0",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    let mut options = self::options(&world, &p2);
    options.tag = "v0.7.0".to_string();
    options.revoke = vec![b1.id];
    prepare(&mut world, &options).unwrap();
    let manifest =
        Manifest::parse(&std::fs::read(world.out.path().join("latest.json")).unwrap()).unwrap();
    let mut revoked = vec![p1.id.to_text(), b1.id.to_text()];
    revoked.sort();
    assert_eq!(manifest.revoked_keys, revoked);
}

// ---------------------------------------------------------------------------------------------
// publish

/// Prepared and signed offline (by the throwaway key, standing in for the official minisign).
fn prepared_and_signed() -> World {
    let mut world = world();
    let options = options(&world, &world.p1);
    prepare(&mut world, &options).unwrap();
    let out = world.out.path();
    let manifest = std::fs::read(out.join("latest.json")).unwrap();
    let comment = std::fs::read_to_string(out.join("trusted-comment.txt")).unwrap();
    std::fs::write(
        out.join("latest.json.minisig"),
        world.p1.sign(&manifest, comment.trim_end()),
    )
    .unwrap();
    // What GitHub serves once v0.2.1 is the latest release.
    let signature = std::fs::read(out.join("latest.json.minisig")).unwrap();
    let mut web = FakeWeb::default();
    web.latest_release(
        "v0.2.1",
        &[
            ("latest.json", &manifest),
            ("latest.json.minisig", &signature),
        ],
    );
    for (name, content) in draft_assets("0.2.1") {
        web.file(&format!("/releases/download/v0.2.1/{name}"), &content);
    }
    world.web = web;
    world
}

fn run_publish(world: &mut World, tag: &str) -> anyhow::Result<String> {
    let mut printed = Vec::new();
    let dir = world.out.path().to_path_buf();
    let mut env = Env {
        host: &mut world.host,
        repo: &mut world.repo,
        clock: &mut world.clock,
        transport: &mut world.web,
        out: &mut printed,
    };
    let result = publish(&mut env, tag, &dir);
    let printed = String::from_utf8(printed).unwrap();
    result.map(|()| printed)
}

#[test]
fn a_release_is_published() {
    let mut world = prepared_and_signed();
    let printed = run_publish(&mut world, "v0.2.1").unwrap();
    assert!(printed.contains("publish: ok"), "{printed}");
    assert_eq!(
        world.host.uploads,
        vec![(
            "v0.2.1".to_string(),
            vec!["latest.json".to_string(), "latest.json.minisig".to_string()]
        )]
    );
    assert_eq!(world.host.publishes, vec!["v0.2.1".to_string()]);
    assert!(world.clock.slept.is_empty());
}

#[test]
fn publish_waits_for_the_cdn() {
    let mut world = prepared_and_signed();
    world.web.fail_first = 1;
    run_publish(&mut world, "v0.2.1").unwrap();
    assert_eq!(world.clock.slept, vec![std::time::Duration::from_secs(30)]);
    // Never served: after 5 minutes, the runbook and a failure (after publishing).
    let mut world = prepared_and_signed();
    world.web.fail_first = 1_000;
    let error = run_publish(&mut world, "v0.2.1").unwrap_err();
    assert!(error.to_string().contains("after publishing"), "{error}");
    assert_eq!(world.host.publishes.len(), 1);
    assert_eq!(world.clock.slept.len(), 10);
}

#[test]
fn publish_refusals() {
    type Change = Box<dyn Fn(&mut World)>;
    // (case, tag, a part of the expected error, the change).
    let cases: Vec<(&str, &str, &str, Change)> = vec![
        (
            "a pre-release tag",
            "v0.2.1-rc.1",
            "pre-release tag",
            Box::new(|_| {}),
        ),
        (
            "another tag than prepared",
            "v0.2.2",
            "was prepared for",
            Box::new(|w| {
                w.host
                    .tag_commits
                    .insert("v0.2.2".to_string(), COMMIT.to_string());
            }),
        ),
        (
            "the draft's digest changed",
            "v0.2.1",
            "changed on GitHub",
            Box::new(|w| {
                w.host.views.get_mut("v0.2.1").unwrap().assets[0].digest =
                    Some(format!("sha256:{}", "1".repeat(64)));
            }),
        ),
        (
            "the tag moved",
            "v0.2.1",
            "when prepared",
            Box::new(|w| {
                w.host
                    .tag_commits
                    .insert("v0.2.1".to_string(), "c".repeat(40));
            }),
        ),
        (
            "the signature does not verify",
            "v0.2.1",
            "would refuse",
            Box::new(|w| {
                let out = w.out.path();
                let manifest = std::fs::read(out.join("latest.json")).unwrap();
                let comment = std::fs::read_to_string(out.join("trusted-comment.txt")).unwrap();
                std::fs::write(
                    out.join("latest.json.minisig"),
                    Key::new().sign(&manifest, comment.trim_end()),
                )
                .unwrap();
            }),
        ),
        (
            "another trusted comment",
            "v0.2.1",
            "trusted comment",
            Box::new(|w| {
                let out = w.out.path();
                let manifest = std::fs::read(out.join("latest.json")).unwrap();
                std::fs::write(
                    out.join("latest.json.minisig"),
                    w.p1.sign(&manifest, "mklm-latest-json v1 version=0.2.1"),
                )
                .unwrap();
            }),
        ),
        (
            "latest.json changed after signing",
            "v0.2.1",
            "would refuse",
            Box::new(|w| {
                let path = w.out.path().join("latest.json");
                let mut manifest = std::fs::read(&path).unwrap();
                manifest.push(b'\n');
                std::fs::write(path, manifest).unwrap();
            }),
        ),
        (
            "the uploaded files did not arrive",
            "v0.2.1",
            "after uploading",
            Box::new(|w| w.host.lose_uploads = true),
        ),
    ];
    for (name, tag, expected, change) in &cases {
        let mut world = prepared_and_signed();
        change(&mut world);
        let error = format!("{:#}", run_publish(&mut world, tag).unwrap_err());
        assert!(error.contains(expected), "{name}: {error}");
        assert!(world.host.publishes.is_empty(), "{name}: never published");
    }
}

// ---------------------------------------------------------------------------------------------
// prepare-release --dev

struct Dist {
    dir: TempDir,
    dev: Key,
}

fn dist(only_x64: bool) -> Dist {
    let dir = TempDir::new("dist-dev");
    let dev = Key::new();
    let mut sums = String::new();
    for (arch, content) in [
        (Arch::X64, &b"dev x64"[..]),
        (Arch::Arm64, &b"dev arm64"[..]),
    ] {
        if only_x64 && arch == Arch::Arm64 {
            continue;
        }
        let name = installer_name(&Version::new(0, 2, 1), arch);
        std::fs::write(dir.path().join(&name), content).unwrap();
        sums.push_str(&format!(
            "{}  {name}\r\n",
            mklm_update::Sha256Digest::of(content).to_hex()
        ));
    }
    std::fs::write(dir.path().join("SHA256SUMS"), sums).unwrap();
    std::fs::write(dir.path().join("mklm-dev.pub"), dev.pub_file()).unwrap();
    Dist { dir, dev }
}

fn dev_options(dist: &Dist) -> PrepareDev {
    PrepareDev {
        tag: "v0.2.1".to_string(),
        dist: dist.dir.path().to_path_buf(),
        dev_pub: dist.dir.path().join("mklm-dev.pub"),
        minisign: Path::new(r"C:\Users\me\mklm-dev-keys\minisign.exe").to_path_buf(),
        only_arch: None,
        expires_days: 180,
        issued_at: None,
        out: dist.dir.path().to_path_buf(),
    }
}

#[test]
fn a_rehearsal_manifest() {
    let dist = dist(false);
    let options = dev_options(&dist);
    let mut out = Vec::new();
    prepare_release_dev(&options, Err(KeyError::NotConfigured), NOW, &mut out).unwrap();
    let dir = dist.dir.path();
    let manifest = Manifest::parse(&std::fs::read(dir.join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest.issued_at, NOW);
    assert_eq!(manifest.expires, NOW + 180 * DAY);
    assert_eq!(manifest.key_ids, vec![dist.dev.id.to_text()]);
    assert!(manifest.revoked_keys.is_empty());
    let comment = std::fs::read_to_string(dir.join("trusted-comment.txt")).unwrap();
    assert_eq!(
        comment,
        format!("mklm-dev-latest-json v1 version=0.2.1 issued_at={NOW}\n")
    );
    let sign = std::fs::read_to_string(dir.join("SIGN-OFFLINE.txt")).unwrap();
    assert_eq!(sign.lines().count(), 1);
    let key_file = dir.join("mklm-dev.key");
    assert!(
        sign.starts_with(&format!(
            r"C:\Users\me\mklm-dev-keys\minisign.exe -S -s {} -m ",
            shell_arg(&key_file)
        )),
        "{sign}"
    );
    assert!(
        sign.trim_end()
            .ends_with(&format!("-t \"{}\"", comment.trim_end()))
    );
    // No prepare.json (publish never handles rehearsals); the .key was never created or read.
    assert!(!dir.join("prepare.json").exists());
    assert!(!key_file.exists());
    // --issued-at (with --expires-days) makes an expired manifest for the rehearsal of the
    // expiry display (design m5b B.3; BUILD-RUN-1). It cannot rehearse a rollback: the
    // development key records nothing (mklm-update's
    // the_development_key_signs_rehearsals_only_and_records_nothing).
    let mut options = dev_options(&dist);
    options.issued_at = Some(NOW - 3 * DAY);
    options.expires_days = 1;
    prepare_release_dev(&options, Err(KeyError::NotConfigured), NOW, &mut out).unwrap();
    let manifest = Manifest::parse(&std::fs::read(dir.join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest.issued_at, NOW - 3 * DAY);
    assert_eq!(manifest.expires, NOW - 2 * DAY);
    assert!(manifest.expires < NOW);
    let comment = std::fs::read_to_string(dir.join("trusted-comment.txt")).unwrap();
    assert_eq!(
        comment,
        format!(
            "mklm-dev-latest-json v1 version=0.2.1 issued_at={}\n",
            NOW - 3 * DAY
        )
    );
}

/// BUILD-RUN-2: the rehearsal's SIGN-OFFLINE.txt line and the printed one paste into PowerShell
/// with paths that contain spaces (the repository lives under "D:\SHIN DATA CENTER\").
#[cfg(windows)]
#[test]
fn a_rehearsal_command_with_spaces_pastes_into_powershell() {
    let dist = dist(false);
    let mut options = dev_options(&dist);
    options.out = dist.dir.path().join("space test");
    options.minisign = Path::new(r"C:\Users\John Smith\mklm-dev-keys\minisign.exe").to_path_buf();
    let mut printed = Vec::new();
    prepare_release_dev(&options, Err(KeyError::NotConfigured), NOW, &mut printed).unwrap();
    let sign = std::fs::read_to_string(options.out.join("SIGN-OFFLINE.txt")).unwrap();
    let comment = std::fs::read_to_string(options.out.join("trusted-comment.txt")).unwrap();
    let expected = vec![
        options.minisign.display().to_string(),
        "-S".to_string(),
        "-s".to_string(),
        dist.dir.path().join("mklm-dev.key").display().to_string(),
        "-m".to_string(),
        options.out.join("latest.json").display().to_string(),
        "-x".to_string(),
        options
            .out
            .join("latest.json.minisig")
            .display()
            .to_string(),
        "-t".to_string(),
        comment.trim_end().to_string(),
    ];
    assert_eq!(powershell_command_elements(&sign), vec![expected.clone()]);
    let printed = String::from_utf8(printed).unwrap();
    let line = printed
        .lines()
        .find(|line| line.contains(" -S -s "))
        .unwrap();
    assert_eq!(powershell_command_elements(line), vec![expected]);
}

/// BUILD-RUN-2: the production SIGN-OFFLINE.txt with an --out folder that contains a space: both
/// lines paste into PowerShell; the fixed media paths stay bare.
#[cfg(windows)]
#[test]
fn signing_commands_with_spaces_paste_into_powershell() {
    let mut world = world();
    let mut options = options(&world, &world.p1);
    options.out = world.out.path().join(r"release work\v0.2.1");
    options.alt_key_id = Some(world.b1.id);
    prepare(&mut world, &options).unwrap();
    let sign = std::fs::read_to_string(options.out.join("SIGN-OFFLINE.txt")).unwrap();
    assert!(
        sign.starts_with(r"E:\tools\minisign.exe -S -s E:\mklm-keys\mklm-primary.key -m '"),
        "{sign}"
    );
    let comment = std::fs::read_to_string(options.out.join("trusted-comment.txt")).unwrap();
    let line = |minisign: &str, key: &str, signature: &str| {
        vec![
            minisign.to_string(),
            "-S".to_string(),
            "-s".to_string(),
            key.to_string(),
            "-m".to_string(),
            options.out.join("latest.json").display().to_string(),
            "-x".to_string(),
            options.out.join(signature).display().to_string(),
            "-t".to_string(),
            comment.trim_end().to_string(),
        ]
    };
    assert_eq!(
        powershell_command_elements(&sign),
        vec![
            line(
                r"E:\tools\minisign.exe",
                r"E:\mklm-keys\mklm-primary.key",
                "latest.json.minisig"
            ),
            line(
                r"F:\tools\minisign.exe",
                r"F:\mklm-keys-backup\mklm-backup.key",
                "latest.json.alt.minisig"
            ),
        ]
    );
}

#[test]
fn a_rehearsal_with_one_architecture() {
    let dist = dist(true);
    let mut options = dev_options(&dist);
    options.only_arch = Some(Arch::X64);
    prepare_release_dev(&options, Err(KeyError::NotConfigured), NOW, &mut Vec::new()).unwrap();
    let manifest =
        Manifest::parse(&std::fs::read(dist.dir.path().join("latest.json")).unwrap()).unwrap();
    let arm64 = manifest
        .assets
        .iter()
        .find(|a| a.arch == Arch::Arm64)
        .unwrap();
    assert_eq!(arm64.size, 1);
    assert_eq!(arm64.sha256, "0".repeat(64));
    // Without --only-arch the missing ARM64 installer is an error.
    let options = dev_options(&dist);
    assert!(
        prepare_release_dev(&options, Err(KeyError::NotConfigured), NOW, &mut Vec::new()).is_err()
    );
}

#[test]
fn rehearsal_refusals() {
    // The production key as the development key.
    let dist = dist(false);
    let production = TrustAnchors::from_keys(&[(Primary, dist.dev.public.as_str())], &[]);
    assert!(prepare_release_dev(&dev_options(&dist), production, NOW, &mut Vec::new()).is_err());
    // A pre-release tag.
    let mut options = dev_options(&dist);
    options.tag = "v0.2.1-dev.1".to_string();
    assert!(
        prepare_release_dev(&options, Err(KeyError::NotConfigured), NOW, &mut Vec::new()).is_err()
    );
    // SHA256SUMS disagrees with a file.
    std::fs::write(dist.dir.path().join("MKLM-Setup-0.2.1-x64.exe"), "changed").unwrap();
    assert!(
        prepare_release_dev(
            &dev_options(&dist),
            Err(KeyError::NotConfigured),
            NOW,
            &mut Vec::new()
        )
        .is_err()
    );
    // A missing file.
    let dist = self::dist(false);
    std::fs::remove_file(dist.dir.path().join("SHA256SUMS")).unwrap();
    assert!(
        prepare_release_dev(
            &dev_options(&dist),
            Err(KeyError::NotConfigured),
            NOW,
            &mut Vec::new()
        )
        .is_err()
    );
    // A malformed committed anchors file stops it too.
    let dist = self::dist(false);
    assert!(
        prepare_release_dev(
            &dev_options(&dist),
            Err(KeyError::BadAnchorsLine { line: 2 }),
            NOW,
            &mut Vec::new()
        )
        .is_err()
    );
    // Not a public key file.
    std::fs::write(dist.dir.path().join("mklm-dev.pub"), "not a key").unwrap();
    assert!(
        prepare_release_dev(
            &dev_options(&dist),
            Err(KeyError::NotConfigured),
            NOW,
            &mut Vec::new()
        )
        .is_err()
    );
}
