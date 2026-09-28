//! Development builds only (`cfg(all(debug_assertions, mklm_update_dev))`, design m5b A.10): the
//! development key and the marker release.yml looks for.
//!
//! The marker must reach every development executable: `#[used]` alone does not promise that a
//! static of an rlib's private module survives the linker's `/OPT:REF`, so [`extra_key`], which
//! `TrustAnchors::for_this_build` calls on every product path, references it through
//! `black_box` (FIX-VERIFICATION-3). The positive control (ci.yml's F.3 step and
//! `build-installer.ps1 -Profile dev`) fails when it is missing.

/// Searched for in the executables: present in development builds, absent in release builds.
#[used]
static DEV_MARKER: [u8; 26] = *b"MKLM-UPDATE-DEV-OVERRIDES!";

/// `MKLM_UPDATE_DEV_PUBKEY` at build time (the base64 line of a minisign `.pub`), if set.
pub(crate) fn extra_key() -> Option<&'static str> {
    let _ = std::hint::black_box(&DEV_MARKER);
    option_env!("MKLM_UPDATE_DEV_PUBKEY")
}
