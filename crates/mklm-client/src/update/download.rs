//! The installer download into the user's cache (design m5b A.8, C.8, E.1).
//!
//! The installer of the verified version and this build's architecture only
//! (`…/releases/download/v<version>/<name>`), into `<name>.part`: `download_asset` checks the
//! exact size and the SHA-256 of the verified manifest while it streams, and only a file that
//! matched is renamed to `<name>`. A failed or cancelled download leaves nothing behind. An
//! installer already in the cache whose size and SHA-256 match is not downloaded again.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mklm_update::fetch::{self, FetchError, Limits, Transport};

use crate::update::cache::UpdateCache;
use crate::update::check::Offer;
use crate::update::env::UpdateEnv;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Fetch(FetchError),
    Cache(String),
}

impl std::fmt::Display for DownloadError {
    /// English, for logs and the technical details.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Fetch(error) => write!(f, "downloading the installer failed: {error}"),
            DownloadError::Cache(detail) => write!(f, "the update cache: {detail}"),
        }
    }
}

/// Into `<name>.part`, verified, then renamed to `<name>`; `progress(received, total)`.
pub fn download(
    transport: &mut dyn Transport,
    env: &UpdateEnv,
    cache: &UpdateCache,
    offer: &Offer,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, DownloadError> {
    download_with(
        &mut |sink, received, cancel| {
            fetch::download_asset(
                transport,
                &env.endpoints,
                &offer.verified.version,
                &offer.verified.asset,
                &Limits::installer(),
                sink,
                received,
                cancel,
            )
        },
        cache,
        offer,
        progress,
        cancel,
    )
}

/// The streaming download: `fetch(sink, received, cancel)`.
pub(crate) type Fetch<'a> =
    dyn FnMut(&mut dyn Write, &mut dyn FnMut(u64), &AtomicBool) -> Result<(), FetchError> + 'a;

/// [`download`] over any fetch.
pub(crate) fn download_with(
    fetch: &mut Fetch<'_>,
    cache: &UpdateCache,
    offer: &Offer,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, DownloadError> {
    let asset = &offer.verified.asset;
    if let Some(path) = cache.matching_installer(asset) {
        progress(asset.size, asset.size);
        return Ok(path);
    }
    let cache_error = |error: std::io::Error| DownloadError::Cache(error.to_string());
    fs::create_dir_all(cache.dir()).map_err(cache_error)?;
    let partial = cache.partial_path(&asset.name);
    let target = cache.installer_path(&asset.name);
    let file = fs::File::create(&partial).map_err(cache_error)?;
    let mut sink = BufWriter::new(file);
    let fetched = fetch(
        &mut sink,
        &mut |received| progress(received, asset.size),
        cancel,
    );
    let written = sink
        .into_inner()
        .map_err(|error| error.into_error())
        .and_then(|file| file.sync_all());
    let result = match (fetched, written) {
        (Err(error), _) => Err(DownloadError::Fetch(error)),
        (Ok(()), Err(error)) => Err(cache_error(error)),
        (Ok(()), Ok(())) => fs::rename(&partial, &target).map_err(cache_error),
    };
    match result {
        Ok(()) => Ok(target),
        Err(error) => {
            let _ = fs::remove_file(&partial);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use mklm_update::{OfferKind, SignatureSlot, Version};

    use super::*;
    use crate::update::cache::tests::Scratch;
    use crate::update::check::tests::{installer, verified};

    fn offer() -> Offer {
        Offer {
            verified: verified("0.2.1", OfferKind::Newer),
            manifest: b"m".to_vec(),
            signature: b"s".to_vec(),
            slot: SignatureSlot::Main,
            skipped: false,
            downloaded: None,
        }
    }

    #[test]
    fn a_verified_download_is_renamed_into_place() {
        let scratch = Scratch::new("download");
        let cache = scratch.cache();
        let (bytes, asset) = installer(&Version::new(0, 2, 1));
        let mut seen = Vec::new();
        let path = download_with(
            &mut |sink, received, _cancel| {
                for (index, chunk) in bytes.chunks(65_536).enumerate() {
                    sink.write_all(chunk).unwrap();
                    received(((index * 65_536) + chunk.len()) as u64);
                }
                Ok(())
            },
            &cache,
            &offer(),
            &mut |received, total| seen.push((received, total)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(path, cache.installer_path(&asset.name));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!cache.partial_path(&asset.name).exists());
        assert_eq!(seen.last(), Some(&(asset.size, asset.size)));
        // Already there: not downloaded again.
        let again = download_with(
            &mut |_, _, _| panic!("downloaded again"),
            &cache,
            &offer(),
            &mut |_, _| {},
            &AtomicBool::new(false),
        );
        assert_eq!(again, Ok(path));
    }

    #[test]
    fn a_failed_download_leaves_nothing() {
        let scratch = Scratch::new("download-fail");
        let cache = scratch.cache();
        let asset = offer().verified.asset;
        for error in [FetchError::HashMismatch, FetchError::Cancelled] {
            let result = download_with(
                &mut |sink, _, _| {
                    sink.write_all(b"partial").unwrap();
                    Err(error.clone())
                },
                &cache,
                &offer(),
                &mut |_, _| {},
                &AtomicBool::new(false),
            );
            assert_eq!(result, Err(DownloadError::Fetch(error)));
            assert!(!cache.partial_path(&asset.name).exists());
            assert!(!cache.installer_path(&asset.name).exists());
        }
        assert!(
            DownloadError::Cache("disk full".into())
                .to_string()
                .contains("disk full")
        );
    }
}
