//! H1's receiving state machine (design m5b D.4 steps 13–14): the installer arrives over the pipe
//! in chunks of at most [`CHUNK_LEN`] bytes, in order, up to the verified size, then its SHA-256 is
//! compared with the manifest.
//!
//! WP-0 writes the types; WP-U the checks.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use crate::Version;
use crate::manifest::{Sha256Digest, Sha256Stream};
use crate::refusal::UpdateRefusal;
use crate::run::RunId;
use crate::verify::{SelectedAsset, VerifiedManifest};

/// Raw bytes per `InstallerChunk` (hex doubles it; fits `MAX_FRAME_LEN`).
pub const CHUNK_LEN: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePlan {
    pub run_id: RunId,
    pub from_version: Version,
    pub to_version: Version,
    pub asset: SelectedAsset,
}

impl StagePlan {
    pub fn new(verified: &VerifiedManifest, from_version: &Version, run_id: RunId) -> StagePlan {
        StagePlan {
            run_id,
            from_version: from_version.clone(),
            to_version: verified.version.clone(),
            asset: verified.asset.clone(),
        }
    }
}

/// H1's receiving state machine (design m5b D.4 steps 13–14). Pure.
#[derive(Debug)]
pub struct Stager {
    plan: StagePlan,
    received: u64,
    hash: Sha256Stream,
}

impl Stager {
    pub fn new(plan: StagePlan) -> Stager {
        Stager {
            plan,
            received: 0,
            hash: Sha256Stream::new(),
        }
    }

    pub fn plan(&self) -> &StagePlan {
        &self.plan
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    /// `offset` must equal `received()`; 1..=CHUNK_LEN bytes; the total may not exceed the size.
    /// Returns the new total. `ChunkOutOfOrder` / `ChunkMalformed` / `InstallerSizeMismatch`.
    pub fn accept(&mut self, offset: u64, data: &[u8]) -> Result<u64, UpdateRefusal> {
        Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
    }

    pub fn is_complete(&self) -> bool {
        self.received == self.plan.asset.size
    }

    /// `InstallerSizeMismatch` / `InstallerHashMismatch`.
    pub fn finish(self) -> Result<Sha256Digest, UpdateRefusal> {
        Err(UpdateRefusal::skeleton()) // Skeleton (M5b): WP-U
    }
}
