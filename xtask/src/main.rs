//! `cargo xtask <command>`: the maintainer's tool for keys and releases (design
//! docs/design/m5b-updater.md B.3, B.5, B.6, F.6, F.8).
//!
//! It handles public data only: it never opens a secret key file and never asks for a password.
//! Signing is done offline by the official minisign binary (design m5b B.1, B.5; SECURITY-2);
//! xtask checks everything before signing (`prepare-release`) and after it (`publish`).
//! Verification and fetches use the product's own code (`mklm-update`). GitHub is reached through
//! the `gh` command behind the `ReleaseHost` trait and git through `Repo`, which tests replace
//! with fakes.
//!
//! Production commands refuse to run in a build with `--cfg mklm_update_dev` (design m5b A.10);
//! the rehearsal commands (`prepare-release --dev`, `serve-releases`) take the development key as
//! a file and never touch GitHub.

mod common;
mod drill;
#[cfg(test)]
mod flow_tests;
mod host;
mod keys;
mod prepare;
mod publish;
mod releases;
mod remote;
mod serve;
mod sums;
#[cfg(test)]
mod testing;
mod time;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use mklm_update::fetch::{Response, Timeouts, Transport, TransportError};
use mklm_update::keys::ANCHORS_TEXT;
use mklm_update::url::Url;
use mklm_update::{Arch, KeyId, KeyRole, TrustAnchors};

use crate::common::Env;
use crate::host::{GhCli, GitCli, SystemClock};

#[derive(Debug, Parser)]
#[command(
    name = "xtask",
    about = "MKLM maintainer tool: key and release checks on public data only (design m5b B.3)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[allow(
    clippy::large_enum_variant,
    reason = "parsed once per run; boxing clap's arguments buys nothing"
)]
enum Command {
    /// Checks crates/mklm-update/trust/anchors.txt against every earlier tag (release.yml).
    CheckKeys,
    /// Prints the trust anchors line and the fingerprint of an official `minisign -G` .pub file.
    PubkeyLine {
        #[arg(long = "pub", value_name = "FILE")]
        public_key: PathBuf,
        #[arg(long, value_enum)]
        role: Role,
    },
    /// Verifies the official minisign release zip with its author's key and prints its SHA-256.
    VerifySigner {
        #[arg(long)]
        zip: PathBuf,
        #[arg(long)]
        sig: PathBuf,
    },
    /// Every check before signing, then latest.json, the trusted comment and SIGN-OFFLINE.txt
    /// (`--dev`: the debug-build rehearsal of design m5b F.6).
    PrepareRelease(PrepareRelease),
    /// Verifies the signatures, uploads them to the draft, publishes it and checks the result.
    Publish {
        #[arg(long)]
        tag: String,
        #[arg(long)]
        dir: PathBuf,
    },
    /// Verifies the published manifest over the production path, or local files (`--dir`).
    Verify(Verify),
    /// Fetches releases/latest/download/SHA256SUMS over the production path (design m5b F.8).
    FetchSmoke,
    /// The yearly backup key drill (design m5b B.6).
    #[command(subcommand)]
    KeyDrill(KeyDrill),
    /// Serves a dist-dev folder on 127.0.0.1 like GitHub's release downloads (rehearsal only).
    ServeReleases {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        port: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Role {
    Primary,
    Backup,
}

impl From<Role> for KeyRole {
    fn from(role: Role) -> KeyRole {
        match role {
            Role::Primary => KeyRole::Primary,
            Role::Backup => KeyRole::Backup,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ArchArg {
    X64,
    Arm64,
}

impl From<ArchArg> for Arch {
    fn from(arch: ArchArg) -> Arch {
        match arch {
            ArchArg::X64 => Arch::X64,
            ArchArg::Arm64 => Arch::Arm64,
        }
    }
}

/// Both grammars of design m5b B.3; which options go with `--dev` is checked by
/// [`PrepareRelease::mode`].
#[derive(Debug, Args)]
struct PrepareRelease {
    /// The debug-build rehearsal (design m5b F.6): `--dist`, `--dev-pub`, `--minisign`.
    #[arg(long)]
    dev: bool,
    #[arg(long)]
    tag: String,
    #[arg(long)]
    commit: Option<String>,
    #[arg(long)]
    main_key_id: Option<String>,
    #[arg(long)]
    alt_key_id: Option<String>,
    /// Default: `mklm_update::DEFAULT_VALIDITY_DAYS` (180, user decision J-3).
    #[arg(long)]
    expires_days: Option<u64>,
    #[arg(long)]
    revoke: Vec<String>,
    #[arg(long)]
    min_from_version: Option<String>,
    #[arg(long, value_delimiter = ',')]
    allow_strand: Vec<String>,
    #[arg(long)]
    published_misdated: bool,
    #[arg(long)]
    dist: Option<PathBuf>,
    #[arg(long)]
    dev_pub: Option<PathBuf>,
    #[arg(long)]
    minisign: Option<PathBuf>,
    #[arg(long, value_enum)]
    only_arch: Option<ArchArg>,
    /// Rehearsal only: this Unix time instead of the local clock, for the expiry display (with
    /// --expires-days 1). Never a rollback: the development key records nothing (design m5b B.3).
    #[arg(long)]
    issued_at: Option<u64>,
    #[arg(long)]
    out: PathBuf,
}

/// The checked options of one grammar.
#[derive(Debug)]
enum PrepareMode {
    Production(prepare::Prepare),
    Rehearsal(prepare::PrepareDev),
}

fn key_id(text: &str, option: &str) -> anyhow::Result<KeyId> {
    KeyId::parse(text).with_context(|| format!("{option} {text:?}: 16 upper-case hex digits"))
}

impl PrepareRelease {
    /// The grammar of the production command and of `--dev` are not mixed (design m5b B.3).
    fn mode(&self) -> anyhow::Result<PrepareMode> {
        let expires_days = self
            .expires_days
            .unwrap_or_else(prepare::default_expires_days);
        if self.dev {
            let production_only = [
                ("--commit", self.commit.is_some()),
                ("--main-key-id", self.main_key_id.is_some()),
                ("--alt-key-id", self.alt_key_id.is_some()),
                ("--revoke", !self.revoke.is_empty()),
                ("--min-from-version", self.min_from_version.is_some()),
                ("--allow-strand", !self.allow_strand.is_empty()),
                ("--published-misdated", self.published_misdated),
            ];
            if let Some((option, _)) = production_only.iter().find(|(_, used)| *used) {
                bail!("{option} is not an option of prepare-release --dev");
            }
            let (Some(dist), Some(dev_pub), Some(minisign)) =
                (&self.dist, &self.dev_pub, &self.minisign)
            else {
                bail!("prepare-release --dev needs --dist, --dev-pub and --minisign");
            };
            return Ok(PrepareMode::Rehearsal(prepare::PrepareDev {
                tag: self.tag.clone(),
                dist: dist.clone(),
                dev_pub: dev_pub.clone(),
                minisign: minisign.clone(),
                only_arch: self.only_arch.map(Arch::from),
                expires_days,
                issued_at: self.issued_at,
                out: self.out.clone(),
            }));
        }
        let rehearsal_only = [
            ("--dist", self.dist.is_some()),
            ("--dev-pub", self.dev_pub.is_some()),
            ("--minisign", self.minisign.is_some()),
            ("--only-arch", self.only_arch.is_some()),
            ("--issued-at", self.issued_at.is_some()),
        ];
        if let Some((option, _)) = rehearsal_only.iter().find(|(_, used)| *used) {
            bail!("{option} belongs to prepare-release --dev (the rehearsal) only");
        }
        let (Some(commit), Some(main_key_id)) = (&self.commit, &self.main_key_id) else {
            bail!("prepare-release needs --commit and --main-key-id");
        };
        Ok(PrepareMode::Production(prepare::Prepare {
            tag: self.tag.clone(),
            commit: commit.clone(),
            main_key_id: key_id(main_key_id, "--main-key-id")?,
            alt_key_id: self
                .alt_key_id
                .as_deref()
                .map(|text| key_id(text, "--alt-key-id"))
                .transpose()?,
            expires_days,
            revoke: self
                .revoke
                .iter()
                .map(|text| key_id(text, "--revoke"))
                .collect::<anyhow::Result<_>>()?,
            min_from_version: self.min_from_version.clone(),
            allow_strand: self.allow_strand.clone(),
            published_misdated: self.published_misdated,
            out: self.out.clone(),
        }))
    }
}

#[derive(Debug, Args)]
struct Verify {
    #[arg(long)]
    remote: bool,
    #[arg(long)]
    installers: bool,
    #[arg(long)]
    min_days_left: Option<u64>,
    #[arg(long)]
    newest_published: bool,
    /// The canary (update-canary.yml): succeed with a notice, without fetching anything, while
    /// no stable vX.Y.Z tag of this repository embeds update keys (before v0.2.0).
    #[arg(long)]
    skip_before_keyed_release: bool,
    #[arg(long)]
    dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum KeyDrill {
    /// Writes a fresh nonce and the offline minisign command.
    Start {
        #[arg(long, value_enum)]
        role: Role,
        #[arg(long)]
        out: PathBuf,
    },
    /// Checks the drill signature against the backup key of every release in the window.
    Check {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, value_enum)]
        role: Role,
    },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::CheckKeys => "check-keys",
            Command::PubkeyLine { .. } => "pubkey-line",
            Command::VerifySigner { .. } => "verify-signer",
            Command::PrepareRelease(_) => "prepare-release",
            Command::Publish { .. } => "publish",
            Command::Verify(_) => "verify",
            Command::FetchSmoke => "fetch-smoke",
            Command::KeyDrill(_) => "key-drill",
            Command::ServeReleases { .. } => "serve-releases",
        }
    }

    /// Everything but the rehearsal commands (design m5b B.3: `prepare-release --dev`,
    /// `serve-releases`).
    fn is_production(&self) -> bool {
        match self {
            Command::PrepareRelease(prepare) => !prepare.dev,
            Command::ServeReleases { .. } => false,
            _ => true,
        }
    }
}

/// The workspace this xtask was built from (git runs there).
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
}

/// The production anchors of this tree (design m5b A.10: never the development key).
fn release_anchors() -> anyhow::Result<TrustAnchors> {
    TrustAnchors::release().context(
        "crates/mklm-update/trust/anchors.txt of this tree has no usable keys (design m5b B.2, B.5)",
    )
}

/// The production fetch path, opened on first use (commands without network never open it).
#[derive(Debug, Default)]
struct LazyTransport {
    #[cfg(windows)]
    inner: Option<mklm_update::winhttp::WinHttpTransport>,
}

impl Transport for LazyTransport {
    #[cfg(windows)]
    fn get(
        &mut self,
        url: &Url,
        accept: &str,
        timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError> {
        if self.inner.is_none() {
            let user_agent =
                mklm_update::user_agent(env!("CARGO_PKG_VERSION"), Arch::of_this_build());
            self.inner = Some(mklm_update::winhttp::WinHttpTransport::new(&user_agent)?);
        }
        match self.inner.as_mut() {
            Some(transport) => transport.get(url, accept, timeouts),
            None => Err(TransportError::Other { code: 0 }),
        }
    }

    #[cfg(not(windows))]
    fn get(
        &mut self,
        _url: &Url,
        _accept: &str,
        _timeouts: &Timeouts,
    ) -> Result<Box<dyn Response + '_>, TransportError> {
        Err(TransportError::Other { code: 50 })
    }
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    if cfg!(mklm_update_dev) && cli.command.is_production() {
        bail!(
            "xtask {} refuses to run: this xtask was built with --cfg mklm_update_dev \
             (design m5b A.10, B.3)",
            cli.command.name()
        );
    }
    let mut stdout = std::io::stdout();
    let mut host = GhCli;
    let mut repo = GitCli::new(workspace_root());
    let mut clock = SystemClock;
    let mut transport = LazyTransport::default();
    let mut env = Env {
        host: &mut host,
        repo: &mut repo,
        clock: &mut clock,
        transport: &mut transport,
        out: &mut stdout,
    };
    match &cli.command {
        Command::CheckKeys => keys::check_keys(env.repo, ANCHORS_TEXT, env.out),
        Command::PubkeyLine { public_key, role } => {
            keys::pubkey_line_command(public_key, (*role).into(), env.out)
        }
        Command::VerifySigner { zip, sig } => keys::verify_signer(zip, sig, env.out),
        Command::PrepareRelease(options) => match options.mode()? {
            PrepareMode::Production(options) => prepare::prepare_release(&mut env, &options),
            PrepareMode::Rehearsal(options) => {
                let now = env.clock.now();
                prepare::prepare_release_dev(&options, TrustAnchors::release(), now, env.out)
            }
        },
        Command::Publish { tag, dir } => publish::publish(&mut env, tag, dir),
        Command::Verify(verify) => match (&verify.dir, verify.remote) {
            (Some(dir), false) => {
                if verify.installers
                    || verify.min_days_left.is_some()
                    || verify.newest_published
                    || verify.skip_before_keyed_release
                {
                    bail!(
                        "--installers, --min-days-left, --newest-published and \
                         --skip-before-keyed-release go with --remote"
                    );
                }
                let now = env.clock.now();
                remote::verify_dir(dir, &release_anchors()?, now, env.out).map(|_| ())
            }
            (None, true) => {
                // Before anything else, even this tree's anchors, which have no key before
                // v0.2.0 (design m5b G.6; BUILD-RUN-4).
                if verify.skip_before_keyed_release
                    && remote::before_keyed_release(env.repo, env.out)?
                {
                    return Ok(());
                }
                let checks = remote::RemoteChecks {
                    installers: verify.installers,
                    min_days_left: verify.min_days_left,
                    newest_published: verify.newest_published,
                    expect_version: None,
                };
                remote::verify_remote(&mut env, &release_anchors()?, &checks).map(|_| ())
            }
            _ => bail!("verify takes either --remote or --dir <dir>"),
        },
        Command::FetchSmoke => remote::fetch_smoke(env.transport, env.out).map(|_| ()),
        Command::KeyDrill(KeyDrill::Start { role, out }) => {
            drill::start((*role).into(), out, env.out)
        }
        Command::KeyDrill(KeyDrill::Check { dir, role }) => {
            drill::check(&mut env, dir, (*role).into()).map(|_| ())
        }
        Command::ServeReleases { dir, port } => {
            serve::serve(dir, port.unwrap_or(serve::DEFAULT_PORT), env.out)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(line: &str) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(line.split_whitespace())
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn commands_parse_as_design_b3_writes_them() {
        for line in [
            "xtask check-keys",
            "xtask pubkey-line --pub mklm-primary.pub --role primary",
            "xtask verify-signer --zip minisign-0.12-win64.zip --sig minisign-0.12-win64.zip.minisig",
            "xtask prepare-release --tag v0.2.1 --commit 0123456789abcdef0123456789abcdef01234567 --main-key-id 8F1A2B3C4D5E6F70 --out release-work",
            "xtask prepare-release --tag v0.2.1 --commit 0123 --main-key-id A --alt-key-id B --expires-days 180 --revoke C --revoke D --min-from-version 0.2.0 --allow-strand v0.2.0,v0.3.0 --published-misdated --out o",
            "xtask prepare-release --dev --tag v0.2.1 --dist dist-dev --dev-pub k.pub --minisign minisign.exe --only-arch x64 --issued-at 1792022400 --out dist-dev",
            "xtask publish --tag v0.2.1 --dir release-work",
            "xtask verify --remote --installers --min-days-left 60 --newest-published",
            "xtask verify --remote --installers --min-days-left 60 --newest-published --skip-before-keyed-release",
            "xtask verify --dir release-work",
            "xtask fetch-smoke",
            "xtask key-drill start --role backup --out drill-2027",
            "xtask key-drill check --dir drill-2027 --role backup",
            "xtask serve-releases --dir dist-dev --port 8421",
        ] {
            parse(line).unwrap_or_else(|error| panic!("{line}: {error}"));
        }
        let cli = parse("xtask prepare-release --tag v0.2.1 --allow-strand v0.2.0,v0.3.0 --out o")
            .unwrap();
        let Command::PrepareRelease(prepare) = cli.command else {
            panic!("prepare-release");
        };
        assert_eq!(prepare.allow_strand, ["v0.2.0", "v0.3.0"]);
        for line in [
            "xtask",
            "xtask sign-release",
            "xtask pubkey-line --pub k.pub --role other",
            "xtask prepare-release --tag v0.2.1",
            "xtask prepare-release --dev --tag v0.2.1 --only-arch x86 --out o",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
    }

    fn mode(line: &str) -> anyhow::Result<PrepareMode> {
        let Command::PrepareRelease(prepare) = parse(line).unwrap().command else {
            panic!("prepare-release");
        };
        prepare.mode()
    }

    #[test]
    fn the_two_grammars_are_not_mixed() {
        let production = "xtask prepare-release --tag v0.2.1 --commit 0123456789abcdef0123456789abcdef01234567 --main-key-id 8F1A2B3C4D5E6F70 --out o";
        let PrepareMode::Production(options) = mode(production).unwrap() else {
            panic!("production");
        };
        assert_eq!(options.expires_days, 180);
        assert_eq!(options.main_key_id.to_text(), "8F1A2B3C4D5E6F70");
        let rehearsal = "xtask prepare-release --dev --tag v0.2.1 --dist d --dev-pub k.pub --minisign m.exe --out d";
        let PrepareMode::Rehearsal(options) = mode(rehearsal).unwrap() else {
            panic!("rehearsal");
        };
        assert_eq!(options.expires_days, 180);
        assert_eq!(options.only_arch, None);
        for extra in [
            "--dist d",
            "--dev-pub k.pub",
            "--minisign m.exe",
            "--only-arch x64",
            "--issued-at 1",
        ] {
            assert!(mode(&format!("{production} {extra}")).is_err(), "{extra}");
        }
        for extra in [
            "--commit 0123456789abcdef0123456789abcdef01234567",
            "--main-key-id 8F1A2B3C4D5E6F70",
            "--alt-key-id 8F1A2B3C4D5E6F70",
            "--revoke 8F1A2B3C4D5E6F70",
            "--min-from-version 0.2.0",
            "--allow-strand v0.2.0",
            "--published-misdated",
        ] {
            assert!(mode(&format!("{rehearsal} {extra}")).is_err(), "{extra}");
        }
        assert!(mode("xtask prepare-release --dev --tag v0.2.1 --dist d --out d").is_err());
        assert!(
            mode("xtask prepare-release --tag v0.2.1 --main-key-id 8F1A2B3C4D5E6F70 --out o")
                .is_err()
        );
        assert!(mode("xtask prepare-release --tag v0.2.1 --commit 0123456789abcdef0123456789abcdef01234567 --main-key-id 8f1a --out o").is_err());
    }

    /// The canary's skip belongs to `--remote` only (BUILD-RUN-4).
    #[test]
    fn the_canary_skip_goes_with_remote_only() {
        for line in [
            "xtask verify --dir d --skip-before-keyed-release",
            "xtask verify --skip-before-keyed-release",
        ] {
            assert!(run(&parse(line).unwrap()).is_err(), "{line}");
        }
    }

    #[test]
    fn rehearsal_commands_are_not_production_commands() {
        let command = |line: &str| parse(line).unwrap().command;
        assert!(command("xtask check-keys").is_production());
        assert!(command("xtask prepare-release --tag v0.2.1 --out o").is_production());
        assert!(!command("xtask prepare-release --dev --tag v0.2.1 --out o").is_production());
        assert!(!command("xtask serve-releases --dir d").is_production());
    }

    /// Design m5b A.10, F.1: in a build with the development cfg, every production command fails
    /// before doing anything.
    #[cfg(mklm_update_dev)]
    #[test]
    fn production_commands_refuse_to_run_in_development_builds() {
        for line in [
            "xtask check-keys",
            "xtask pubkey-line --pub k.pub --role primary",
            "xtask verify-signer --zip z --sig s",
            "xtask prepare-release --tag v0.2.1 --commit 0123456789abcdef0123456789abcdef01234567 --main-key-id 8F1A2B3C4D5E6F70 --out o",
            "xtask publish --tag v0.2.1 --dir d",
            "xtask verify --remote",
            "xtask fetch-smoke",
            "xtask key-drill start --role backup --out d",
            "xtask key-drill check --dir d --role backup",
        ] {
            let error = run(&parse(line).unwrap()).unwrap_err();
            assert!(
                error.to_string().contains("--cfg mklm_update_dev"),
                "{line}: {error}"
            );
        }
    }
}
