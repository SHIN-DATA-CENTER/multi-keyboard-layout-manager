//! The user's update cache and record: `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\`
//! (design m5b C.4, E.1, D.11).
//!
//! WP-C implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::io;
use std::path::PathBuf;

use mklm_update::{SignatureSlot, TrustState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorClass {
    Transient,
    Structural,
}

/// The last failed check (design m5b E.3; OPS-UX-TEST-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub class: ErrorClass,
    /// The message ID of design m5b E.6 (e.g. "upd-gh-changed").
    pub message_id: String,
    /// When this run of failures began (Unix seconds).
    pub first_at: u64,
    pub at: u64,
}

/// The last ignored older manifest (design m5b E.3; SECURITY-11, RELIABILITY-6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackNote {
    pub issued_at: u64,
    pub seen: u64,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientState {
    pub schema: u32, // 1
    #[serde(default)]
    pub trust: TrustState,
    #[serde(default)]
    pub last_check: Option<u64>,
    #[serde(default)]
    pub last_success: Option<u64>,
    #[serde(default)]
    pub last_failure: Option<CheckFailure>,
    #[serde(default)]
    pub last_rollback: Option<RollbackNote>,
    /// Which signature the cached `latest.json.minisig` is.
    #[serde(default)]
    pub cached_signature: Option<SignatureSlot>,
}

impl Default for ClientState {
    /// Schema 1, nothing recorded.
    fn default() -> ClientState {
        ClientState {
            schema: 1,
            trust: TrustState::default(),
            last_check: None,
            last_success: None,
            last_failure: None,
            last_rollback: None,
            cached_signature: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdateCache {
    dir: PathBuf,
}

impl UpdateCache {
    pub fn new(dir: PathBuf) -> UpdateCache {
        UpdateCache { dir }
    }

    /// Missing or unreadable → default.
    pub fn load_state(&self) -> ClientState {
        ClientState::default() // Skeleton (M5b): WP-C
    }

    /// Temporary file, then rename.
    pub fn save_state(&self, state: &ClientState) -> io::Result<()> {
        Err(skeleton()) // Skeleton (M5b): WP-C
    }

    /// The manifest and the signature that verified it.
    pub fn store_manifest(&self, manifest: &[u8], signature: &[u8]) -> io::Result<()> {
        Err(skeleton()) // Skeleton (M5b): WP-C
    }

    pub fn load_manifest(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        None // Skeleton (M5b): WP-C
    }

    pub fn installer_path(&self, name: &str) -> PathBuf {
        self.dir.join(name) // Skeleton (M5b): WP-C checks the name
    }

    pub fn partial_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.part")) // Skeleton (M5b): WP-C checks the name
    }

    /// Deletes every installer and `.part` except `keep_installer`. The GUI calls it only under
    /// the conditions of design m5b D.11.
    pub fn prune(&self, keep_installer: Option<&str>) -> io::Result<()> {
        Err(skeleton()) // Skeleton (M5b): WP-C
    }
}

/// What the WP-0 skeleton's unimplemented functions return (design m5b G.2).
fn skeleton() -> io::Error {
    io::Error::other("not implemented (m5b skeleton)")
}
