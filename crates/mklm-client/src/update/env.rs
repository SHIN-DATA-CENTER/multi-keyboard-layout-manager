//! What this installation can do about updates (design m5b D.2, E.1, E.8): the installed version
//! and architecture, the trust anchors of this build, the installation folder and the user's
//! cache, and from them whether updates are available at all.
//!
//! | Availability | When | What the front ends offer |
//! |---|---|---|
//! | `NotConfigured` | the build has no update keys (`TrustAnchors::for_this_build`) | nothing |
//! | `NotInstalledCopy` | MKLM runs outside `%ProgramFiles%\SHIN DATA CENTER\MKLM` (development builds) | a check only |
//! | `Unknown` | something could not be read | nothing |
//! | `Available` | otherwise | check, download, install |
//!
//! The helper decides again for itself (design m5b D.4 step 1): this is what the front ends show
//! and whether they offer the button.

use std::path::{Path, PathBuf};

use mklm_update::url::Endpoints;
use mklm_update::{Arch, KeyError, TrustAnchors, Version};
use mklm_win::os::NativeMachine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    NotConfigured,
    NotInstalledCopy {
        exe_dir: PathBuf,
        install_dir: PathBuf,
    },
    Unknown {
        detail: String,
    },
}

#[derive(Debug)]
pub struct UpdateEnv {
    pub installed: Version,
    pub arch: Arch,
    pub native: Option<NativeMachine>,
    pub availability: Availability,
    /// `TrustAnchors::for_this_build()`.
    pub anchors: Option<TrustAnchors>,
    pub endpoints: Endpoints,
    pub install_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// `app_version` = the front end's `CARGO_PKG_VERSION`; `endpoints` = `Endpoints::production()`
/// (development builds: maybe the `--update-endpoint` override).
pub fn environment(app_version: &str, endpoints: Endpoints) -> UpdateEnv {
    // `for_this_build` is also what makes a development executable carry the development marker
    // (design m5b A.10; FIX-VERIFICATION-3).
    let anchors = TrustAnchors::for_this_build();
    let installed = Version::parse(app_version);
    let install_dir = mklm_win::os::fixed_install_dir();
    let cache_dir = mklm_win::user_dirs::update_cache_dir();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let availability = availability(
        anchors.as_ref().err(),
        installed.as_ref().err().map(ToString::to_string),
        install_dir.as_ref().map_err(ToString::to_string),
        exe_dir.as_deref(),
        cache_dir.as_ref().err().map(ToString::to_string),
    );
    UpdateEnv {
        installed: installed.unwrap_or(Version::new(0, 0, 0)),
        arch: Arch::of_this_build(),
        native: mklm_win::os::native_machine().ok(),
        availability,
        anchors: anchors.ok(),
        endpoints,
        install_dir: install_dir.unwrap_or_default(),
        cache_dir: cache_dir.unwrap_or_default(),
    }
}

/// The availability from what could be read (pure). The build's keys first: without them nothing
/// can be verified, wherever MKLM runs.
fn availability(
    anchors: Option<&KeyError>,
    version: Option<String>,
    install_dir: Result<&PathBuf, String>,
    exe_dir: Option<&Path>,
    cache_dir: Option<String>,
) -> Availability {
    let unknown = |detail: String| Availability::Unknown { detail };
    match anchors {
        Some(KeyError::NotConfigured) => return Availability::NotConfigured,
        Some(error) => return unknown(format!("the update keys of this build: {error}")),
        None => {}
    }
    if let Some(error) = version {
        return unknown(format!("the version of this build: {error}"));
    }
    let install_dir = match install_dir {
        Ok(dir) => dir,
        Err(error) => return unknown(format!("the installation folder: {error}")),
    };
    let Some(exe_dir) = exe_dir else {
        return unknown("this program's folder could not be read".to_string());
    };
    if !same_dir(exe_dir, install_dir) {
        return Availability::NotInstalledCopy {
            exe_dir: exe_dir.to_path_buf(),
            install_dir: install_dir.clone(),
        };
    }
    if let Some(error) = cache_dir {
        return unknown(format!("the update cache folder: {error}"));
    }
    Availability::Available
}

/// The same folder, compared as Windows does (case-insensitive), through their canonical forms
/// when both can be read.
fn same_dir(a: &Path, b: &Path) -> bool {
    let text = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    if text(a) == text(b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => text(&a) == text(&b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTALL: &str = r"C:\Program Files\SHIN DATA CENTER\MKLM";

    #[test]
    fn keys_first_then_the_folder() {
        let install = PathBuf::from(INSTALL);
        let installed = Path::new(r"c:\program files\shin data center\mklm\");
        let dev = Path::new(r"D:\src\mklm\target\debug");
        assert_eq!(
            availability(None, None, Ok(&install), Some(installed), None),
            Availability::Available
        );
        assert_eq!(
            availability(
                Some(&KeyError::NotConfigured),
                None,
                Ok(&install),
                Some(dev),
                None
            ),
            Availability::NotConfigured
        );
        assert_eq!(
            availability(None, None, Ok(&install), Some(dev), None),
            Availability::NotInstalledCopy {
                exe_dir: dev.to_path_buf(),
                install_dir: install.clone(),
            }
        );
        assert!(matches!(
            availability(
                Some(&KeyError::BadAnchorsLine { line: 3 }),
                None,
                Ok(&install),
                Some(installed),
                None
            ),
            Availability::Unknown { detail } if detail.contains("line 3")
        ));
        assert!(matches!(
            availability(
                None,
                None,
                Err("SHGetKnownFolderPath".into()),
                Some(installed),
                None
            ),
            Availability::Unknown { .. }
        ));
        assert!(matches!(
            availability(None, None, Ok(&install), Some(installed), Some("denied".into())),
            Availability::Unknown { detail } if detail.contains("cache")
        ));
        assert!(matches!(
            availability(
                None,
                Some("bad".into()),
                Ok(&install),
                Some(installed),
                None
            ),
            Availability::Unknown { .. }
        ));
        // Canonical forms: the same folder written differently.
        let here = std::env::current_dir().unwrap();
        assert!(same_dir(&here, &here.join(".")));
        assert!(!same_dir(&here, dev));
    }
}
