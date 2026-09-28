//! The build ID both executables embed (design E.3, review S11), shared by
//! `apps/mklm-cli/build.rs` and `apps/mklm-helper/build.rs` (`#[path]` module), so that the two
//! compute it in exactly the same way.
//!
//! The ID is `<package version>+<32 hex digits>`: the version, and a 128-bit FNV-1a hash of the
//! sources of the crates whose rules and wire format caller and helper must agree on
//! (`mklm-core`, `mklm-ipc`, `mklm-engine`, `mklm-win`, `mklm-update`: each `Cargo.toml` and every
//! file under `src/`), and of the files outside those that the build embeds ([`HASHED_FILES`]: the
//! update trust anchors, design m5b G.2), in sorted order of their relative paths. It catches a
//! change that forgot to bump `mklm_ipc::PROTOCOL_VERSION`, and a helper left over from an older
//! build (`cargo build -p mklm-cli` alone). It is a consistency check between two files of one
//! installation, not a security boundary, so a non-cryptographic hash is enough.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The crates whose sources the ID covers, relative to the workspace root. `mklm-update` because
/// the pipe messages embed its types (design m5b G.2).
pub const HASHED_CRATES: [&str; 5] = [
    "crates/mklm-core",
    "crates/mklm-ipc",
    "crates/mklm-engine",
    "crates/mklm-win",
    "crates/mklm-update",
];

/// Files outside each crate's `Cargo.toml` and `src/` that are embedded in the build
/// (`include_str!`), relative to the workspace root: two builds that differ only in their trusted
/// update keys must not share a build ID (design m5b G.2; FIX-VERIFICATION-15).
pub const HASHED_FILES: [&str; 1] = ["crates/mklm-update/trust/anchors.txt"];

/// FNV-1a, 128-bit (offset basis and prime from the FNV specification).
const FNV_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

struct Fnv(u128);

impl Fnv {
    fn new() -> Self {
        Self(FNV_OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u128::from(byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
}

/// Paths (below `workspace_root`) whose changes must re-run the build script: each hashed crate's
/// `Cargo.toml` and `src` directory (Cargo scans a directory recursively), and each hashed file.
pub fn watched_paths(workspace_root: &Path) -> Vec<PathBuf> {
    HASHED_CRATES
        .iter()
        .flat_map(|krate| {
            let dir = workspace_root.join(krate);
            [dir.join("Cargo.toml"), dir.join("src")]
        })
        .chain(HASHED_FILES.iter().map(|file| workspace_root.join(file)))
        .collect()
}

/// `<version>+<hash>` over the hashed crates and files below `workspace_root`.
pub fn build_id(workspace_root: &Path, version: &str) -> io::Result<String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for krate in HASHED_CRATES {
        let dir = workspace_root.join(krate);
        files.push((format!("{krate}/Cargo.toml"), dir.join("Cargo.toml")));
        collect(&dir.join("src"), &format!("{krate}/src"), &mut files)?;
    }
    for file in HASHED_FILES {
        files.push((file.to_string(), workspace_root.join(file)));
    }
    files.sort();
    let mut hash = Fnv::new();
    for (relative, path) in &files {
        let content = fs::read(path)?;
        hash.write(relative.as_bytes());
        hash.write(&[0]);
        hash.write(&(content.len() as u64).to_le_bytes());
        hash.write(&content);
    }
    Ok(format!("{version}+{:032x}", hash.0))
}

/// Every file below `dir`, with its path relative to the workspace root (`/` separators).
fn collect(dir: &Path, relative: &str, files: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let child = format!("{relative}/{name}");
        if entry.file_type()?.is_dir() {
            collect(&path, &child, files)?;
        } else {
            files.push((child, path));
        }
    }
    Ok(())
}
