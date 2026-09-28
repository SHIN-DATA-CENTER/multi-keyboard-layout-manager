//! H1's receiving state machine (design m5b D.4 steps 13–14): the installer arrives over the pipe
//! in chunks of at most [`CHUNK_LEN`] bytes, in order, up to the verified size, then its SHA-256 is
//! compared with the manifest.

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
    ///
    /// A refused chunk changes nothing; the caller ends the staging on any refusal.
    pub fn accept(&mut self, offset: u64, data: &[u8]) -> Result<u64, UpdateRefusal> {
        if offset != self.received {
            return Err(UpdateRefusal::ChunkOutOfOrder {
                expected: self.received,
                found: offset,
            });
        }
        if data.is_empty() || data.len() > CHUNK_LEN {
            return Err(UpdateRefusal::ChunkMalformed);
        }
        let total = self.received + data.len() as u64;
        if total > self.plan.asset.size {
            return Err(UpdateRefusal::InstallerSizeMismatch {
                expected: self.plan.asset.size,
                received: total,
            });
        }
        self.hash.update(data);
        self.received = total;
        Ok(total)
    }

    pub fn is_complete(&self) -> bool {
        self.received == self.plan.asset.size
    }

    /// `InstallerSizeMismatch` / `InstallerHashMismatch`.
    pub fn finish(self) -> Result<Sha256Digest, UpdateRefusal> {
        if !self.is_complete() {
            return Err(UpdateRefusal::InstallerSizeMismatch {
                expected: self.plan.asset.size,
                received: self.received,
            });
        }
        let digest = self.hash.finish();
        if digest != self.plan.asset.sha256 {
            return Err(UpdateRefusal::InstallerHashMismatch);
        }
        Ok(digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Arch;

    fn stager(content: &[u8]) -> Stager {
        Stager::new(StagePlan {
            run_id: RunId::new(
                &Version::new(0, 2, 1),
                [0x3f, 0x9a, 0x0c, 0x2b, 0x7d, 0x1e, 0x4a, 0x65],
            ),
            from_version: Version::new(0, 2, 0),
            to_version: Version::new(0, 2, 1),
            asset: SelectedAsset {
                arch: Arch::X64,
                name: "MKLM-Setup-0.2.1-x64.exe".to_string(),
                size: content.len() as u64,
                sha256: Sha256Digest::of(content),
            },
        })
    }

    fn content(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn chunks_in_order_up_to_the_size() {
        let data = content(CHUNK_LEN * 2 + 10);
        let mut stager = stager(&data);
        assert_eq!(stager.plan().to_version, Version::new(0, 2, 1));
        let mut offset = 0u64;
        for chunk in data.chunks(CHUNK_LEN) {
            assert!(!stager.is_complete());
            offset = stager.accept(offset, chunk).unwrap();
            assert_eq!(stager.received(), offset);
        }
        assert!(stager.is_complete());
        assert_eq!(stager.finish(), Ok(Sha256Digest::of(&data)));
    }

    #[test]
    fn out_of_order_empty_and_oversized_chunks() {
        let data = content(CHUNK_LEN + 5);
        let mut stager = stager(&data);
        assert_eq!(
            stager.accept(1, &data[..10]),
            Err(UpdateRefusal::ChunkOutOfOrder {
                expected: 0,
                found: 1
            })
        );
        assert_eq!(stager.accept(0, &[]), Err(UpdateRefusal::ChunkMalformed));
        let too_long = vec![0u8; CHUNK_LEN + 1];
        assert_eq!(
            stager.accept(0, &too_long),
            Err(UpdateRefusal::ChunkMalformed)
        );
        // A refused chunk changed nothing.
        assert_eq!(stager.received(), 0);
        assert_eq!(stager.accept(0, &data[..CHUNK_LEN]), Ok(CHUNK_LEN as u64));
        // The same chunk again, and one from the past.
        assert_eq!(
            stager.accept(0, &data[..CHUNK_LEN]),
            Err(UpdateRefusal::ChunkOutOfOrder {
                expected: CHUNK_LEN as u64,
                found: 0
            })
        );
        // More than the verified size.
        assert_eq!(
            stager.accept(CHUNK_LEN as u64, &[1; 6]),
            Err(UpdateRefusal::InstallerSizeMismatch {
                expected: CHUNK_LEN as u64 + 5,
                received: CHUNK_LEN as u64 + 6
            })
        );
        assert_eq!(stager.received(), CHUNK_LEN as u64);
    }

    #[test]
    fn finishing_early_or_with_other_content() {
        let data = content(100);
        let mut early = stager(&data);
        early.accept(0, &data[..99]).unwrap();
        assert_eq!(
            early.finish(),
            Err(UpdateRefusal::InstallerSizeMismatch {
                expected: 100,
                received: 99
            })
        );
        assert_eq!(
            stager(&data).finish(),
            Err(UpdateRefusal::InstallerSizeMismatch {
                expected: 100,
                received: 0
            })
        );
        let mut other = stager(&data);
        let mut changed = data.clone();
        changed[50] ^= 1;
        other.accept(0, &changed).unwrap();
        assert!(other.is_complete());
        assert_eq!(other.finish(), Err(UpdateRefusal::InstallerHashMismatch));
    }
}
