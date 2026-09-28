//! The two things the GUI may hand to the shell for updates (design m5b D.13, E.2, E.7; G.6 review
//! list): the release page, and the cached installer on the user's button press. Nothing else:
//! the URL is a fixed prefix and a strict `X.Y.Z` version, and the installer must be a regular
//! file with the installer's name directly in the user's update cache.

use std::os::windows::fs::MetadataExt as _;
use std::path::Path;

use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::elevation::ComApartment;
use crate::error::{Error, win32_code};
use crate::sys::{last_error, wide_os, win32};

/// `mklm_update::release_page_url` without the version (the GUI's tests compare the two).
pub const RELEASE_TAG_URL_PREFIX: &str =
    "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/tag/v";

/// `ERROR_INVALID_PARAMETER`: what is refused before anything is opened.
const INVALID_PARAMETER: u32 = 87;

/// `X.Y.Z`: three parts of 1 to 5 digits, each `0` or without a leading zero.
pub fn is_release_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 5
                && part.bytes().all(|b| b.is_ascii_digit())
                && (*part == "0" || !part.starts_with('0'))
        })
}

/// `MKLM-Setup-<X.Y.Z>-<x64|arm64>.exe` (`mklm_update::installer_name`).
pub fn is_installer_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("MKLM-Setup-") else {
        return false;
    };
    ["-x64.exe", "-arm64.exe"]
        .iter()
        .any(|suffix| rest.strip_suffix(suffix).is_some_and(is_release_version))
}

/// The release page's URL of `version`, or `None` when it is not `X.Y.Z`.
pub fn release_page(version: &str) -> Option<String> {
    is_release_version(version).then(|| format!("{RELEASE_TAG_URL_PREFIX}{version}"))
}

/// Opens `<repository>/releases/tag/v<version>`; `version` must be `X.Y.Z` digits. Blocking for
/// a moment: call it off the UI thread.
pub fn open_release_page(version: &str) -> Result<(), Error> {
    let url = release_page(version).ok_or(Error::Win32 {
        function: "open_release_page",
        code: INVALID_PARAMETER,
    })?;
    let _com = ComApartment::enter();
    let url = wide_os(std::ffi::OsStr::new(&url));
    if super::shell_open(PCWSTR(url.as_ptr()), PCWSTR::null(), PCWSTR::null()) {
        Ok(())
    } else {
        Err(last_error("ShellExecuteW"))
    }
}

/// `ShellExecuteExW` with `runas` of an installer in the user's update cache, on the user's button
/// press only (design m5b D.13; RELIABILITY-2). The caller's size and SHA-256 check only catches a
/// corrupted file: the signed-in user can swap the file afterwards, so this path is no more
/// trusted than running a downloaded installer by hand (RED-TEAM-2). `Error::Cancelled` when UAC
/// is declined.
///
/// `installer` must be `<update cache>\MKLM-Setup-<X.Y.Z>-<arch>.exe`, a regular file and not a
/// reparse point (`Error::Insecure` otherwise). The installer shows its own pages (no `/S`); its
/// working directory is System32. Blocks until the prompt is answered: call it off the UI thread.
pub fn run_installer_interactive(installer: &Path) -> Result<(), Error> {
    let insecure = |reason: &str| Error::Insecure {
        path: installer.display().to_string(),
        reason: reason.to_string(),
    };
    let named = installer
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_installer_name);
    if !named {
        return Err(insecure("not an MKLM installer's name"));
    }
    let cache = crate::user_dirs::update_cache_path()?;
    let in_cache = installer.parent().is_some_and(|parent| {
        parent
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&cache.as_os_str().to_string_lossy())
    });
    if !installer.is_absolute() || !in_cache {
        return Err(insecure("not in the update cache"));
    }
    let metadata = std::fs::symlink_metadata(installer).map_err(|error| Error::Win32 {
        function: "GetFileAttributesExW",
        code: error.raw_os_error().map_or(0, |code| code as u32),
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(insecure("is a reparse point"));
    }
    if !metadata.is_file() {
        return Err(insecure("is not a regular file"));
    }
    let file = wide_os(installer.as_os_str());
    let directory = wide_os(crate::elevation::system_directory()?.as_os_str());
    let _com = ComApartment::enter();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised with its size; its strings are a static literal or
    // NUL-terminated buffers that outlive the call.
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) => Ok(()),
        Err(error) if win32_code(&error) == ERROR_CANCELLED.0 => Err(Error::Cancelled),
        Err(error) => Err(win32("ShellExecuteExW", &error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_release_versions_make_a_url() {
        assert_eq!(
            release_page("0.2.1").as_deref(),
            Some(
                "https://github.com/SHIN-DATA-CENTER/multi-keyboard-layout-manager/releases/tag/v0.2.1"
            )
        );
        assert!(release_page("65535.0.10").is_some());
        for bad in [
            "",
            "0.2",
            "0.2.1.0",
            "v0.2.1",
            "0.2.1-dev.1",
            "0.2.1+b",
            "00.2.1",
            "0.02.1",
            "123456.0.0",
            "0.2.1/../../evil",
            "0.2.1?x",
            " 0.2.1",
            "0.2.x",
            "٠.2.1",
        ] {
            assert_eq!(release_page(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_installer_names_are_run() {
        assert!(is_installer_name("MKLM-Setup-0.2.1-x64.exe"));
        assert!(is_installer_name("MKLM-Setup-10.0.0-arm64.exe"));
        for bad in [
            "MKLM-Setup-0.2.1-x86.exe",
            "MKLM-Setup-0.2.1-x64.exe.part",
            "MKLM-Setup-0.2-x64.exe",
            "mklm-setup-0.2.1-x64.exe",
            "MKLM-Setup-0.2.1-x64.EXE",
            "MKLM-Setup-0.2.1-x64.bat",
            "MKLM-Setup--x64.exe",
            "..\\MKLM-Setup-0.2.1-x64.exe",
            "evil.exe",
        ] {
            assert!(!is_installer_name(bad), "{bad:?}");
        }
    }

    /// Refused before anything is opened: never runs a program in a test.
    #[test]
    fn a_file_outside_the_cache_is_never_run() {
        let outside = std::env::temp_dir().join("MKLM-Setup-0.2.1-x64.exe");
        assert!(matches!(
            run_installer_interactive(&outside),
            Err(Error::Insecure { .. })
        ));
        assert!(matches!(
            run_installer_interactive(Path::new("MKLM-Setup-0.2.1-x64.exe")),
            Err(Error::Insecure { .. })
        ));
        assert!(open_release_page("0.2.1/../x").is_err());
    }
}
