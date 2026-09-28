//! `cargo xtask <command>`: the maintainer's tool for keys and releases (design
//! docs/design/m5b-updater.md B.3, B.5, B.6, F.6, F.8).
//!
//! It handles public data only: it never opens a secret key file and never asks for a password.
//! Signing is done offline by the official minisign binary (design m5b B.1, B.5; SECURITY-2).
//! Verification and fetches use the product's own code (`mklm-update`). GitHub is reached through
//! the `gh` command behind a `ReleaseHost` trait, which tests replace with a fake.
//!
//! Production commands refuse to run in a build with `--cfg mklm_update_dev` (design m5b A.10).
//!
//! Skeleton (M5b): WP-U implements the commands; each one fails with "not implemented yet".

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ArchArg {
    X64,
    Arm64,
}

/// Both grammars of design m5b B.3; which options go with `--dev` is checked by the command.
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
    #[arg(long)]
    issued_at: Option<u64>,
    #[arg(long)]
    out: PathBuf,
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

fn run(cli: &Cli) -> anyhow::Result<()> {
    if cfg!(mklm_update_dev) && cli.command.is_production() {
        anyhow::bail!(
            "xtask {} refuses to run: this xtask was built with --cfg mklm_update_dev \
             (design m5b A.10, B.3)",
            cli.command.name()
        );
    }
    // Skeleton (M5b): WP-U.
    anyhow::bail!(
        "xtask {}: not implemented yet (m5b skeleton)",
        cli.command.name()
    )
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

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn commands_parse_as_design_b3_writes_them() {
        let parse = |line: &str| Cli::try_parse_from(line.split_whitespace());
        for line in [
            "xtask check-keys",
            "xtask pubkey-line --pub mklm-primary.pub --role primary",
            "xtask verify-signer --zip minisign-0.12-win64.zip --sig minisign-0.12-win64.zip.minisig",
            "xtask prepare-release --tag v0.2.1 --commit 0123456789abcdef0123456789abcdef01234567 --main-key-id 8F1A2B3C4D5E6F70 --out release-work",
            "xtask prepare-release --tag v0.2.1 --commit 0123 --main-key-id A --alt-key-id B --expires-days 180 --revoke C --revoke D --min-from-version 0.2.0 --allow-strand v0.2.0,v0.3.0 --published-misdated --out o",
            "xtask prepare-release --dev --tag v0.2.1 --dist dist-dev --dev-pub k.pub --minisign minisign.exe --only-arch x64 --issued-at 1792022400 --out dist-dev",
            "xtask publish --tag v0.2.1 --dir release-work",
            "xtask verify --remote --installers --min-days-left 60 --newest-published",
            "xtask verify --dir release-work",
            "xtask fetch-smoke",
            "xtask key-drill start --role backup --out drill-2027",
            "xtask key-drill check --dir drill-2027 --role backup",
            "xtask serve-releases --dir dist-dev --port 8421",
        ] {
            let cli = parse(line).unwrap_or_else(|error| panic!("{line}: {error}"));
            // Every command is a skeleton for now, and fails.
            assert!(run(&cli).is_err(), "{line}");
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

    #[test]
    fn rehearsal_commands_are_not_production_commands() {
        let command = |line: &str| {
            Cli::try_parse_from(line.split_whitespace())
                .unwrap()
                .command
        };
        assert!(command("xtask check-keys").is_production());
        assert!(command("xtask prepare-release --tag v0.2.1 --out o").is_production());
        assert!(!command("xtask prepare-release --dev --tag v0.2.1 --out o").is_production());
        assert!(!command("xtask serve-releases --dir d").is_production());
    }
}
