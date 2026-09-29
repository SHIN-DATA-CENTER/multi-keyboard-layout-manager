//! Operating system facts: build number, UBR, native architecture and Remote Desktop.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use mklm_core::{KeyboardType, OsInfo};
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
    IMAGE_FILE_MACHINE_ARMNT, IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_UNKNOWN, OSVERSIONINFOW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardType;
use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
use windows_registry::LOCAL_MACHINE;

use crate::error::{Error, ReadIssue, ReadIssueKind, win32_code};
use crate::reg::{open_read, read_dword};
use crate::sys::win32;

/// Key holding `UBR`, relative to `HKEY_LOCAL_MACHINE`.
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Reads the OS facts. Only a failing `RtlGetVersion` is an error; a missing UBR is `None` and an
/// unknown architecture falls back to the one this binary was built for, with an issue.
pub fn read_os_info(issues: &mut Vec<ReadIssue>) -> Result<OsInfo, Error> {
    // SAFETY: GetSystemMetrics has no preconditions.
    let remote_session = unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0;
    Ok(OsInfo {
        build: build_number()?,
        ubr: ubr(issues),
        native_arch: native_arch(issues),
        remote_session,
        client_keyboard_type: remote_session.then(session_keyboard_type).flatten(),
    })
}

/// The keyboard type the Remote Desktop client reported (`OsInfo::client_keyboard_type`):
/// `GetKeyboardType` type and subtype, `None` when the type is 0 (the call failed). Read only in
/// a remote session: on the console it describes no particular keyboard among several.
fn session_keyboard_type() -> Option<KeyboardType> {
    // SAFETY: GetKeyboardType has no preconditions; 0 asks for the type.
    let ty = u32::try_from(unsafe { GetKeyboardType(0) }).ok()?;
    if ty == 0 {
        return None;
    }
    // SAFETY: as above; 1 asks for the subtype, for which 0 is a valid answer.
    let subtype = u32::try_from(unsafe { GetKeyboardType(1) }).unwrap_or(0);
    Some(KeyboardType::new(ty, subtype))
}

/// Build number from `RtlGetVersion`, which, unlike `GetVersionEx`, is not affected by manifests.
fn build_number() -> Result<u32, Error> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a writable OSVERSIONINFOW whose size field is set, as the API requires.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_err() {
        return Err(Error::NtStatus {
            function: "RtlGetVersion",
            status: status.0 as u32,
        });
    }
    Ok(info.dwBuildNumber)
}

fn ubr(issues: &mut Vec<ReadIssue>) -> Option<u32> {
    let path = format!(r"HKLM\{CURRENT_VERSION}");
    let result = open_read(LOCAL_MACHINE, "HKLM", CURRENT_VERSION).and_then(|key| match key {
        Some(key) => read_dword(&key, &path, "UBR"),
        None => Ok(None),
    });
    result.unwrap_or_else(|error| {
        issues.push(ReadIssue::new(
            ReadIssueKind::Environment,
            format!(r"{path}\UBR"),
            error,
        ));
        None
    })
}

fn native_arch(issues: &mut Vec<ReadIssue>) -> String {
    let mut process = IMAGE_FILE_MACHINE_UNKNOWN;
    let mut native = IMAGE_FILE_MACHINE_UNKNOWN;
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing; both out pointers
    // are valid for the call.
    let result = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) };
    match result {
        Ok(()) if native != IMAGE_FILE_MACHINE_UNKNOWN => machine_name(native),
        Ok(()) => build_arch().to_string(),
        Err(error) => {
            issues.push(ReadIssue::new(
                ReadIssueKind::Environment,
                "IsWow64Process2",
                Error::Win32 {
                    function: "IsWow64Process2",
                    code: win32_code(&error),
                },
            ));
            build_arch().to_string()
        }
    }
}

/// Architecture name used in `OsInfo::native_arch`.
pub(crate) fn machine_name(machine: IMAGE_FILE_MACHINE) -> String {
    match machine {
        IMAGE_FILE_MACHINE_AMD64 => "x64".to_string(),
        IMAGE_FILE_MACHINE_ARM64 => "arm64".to_string(),
        IMAGE_FILE_MACHINE_I386 => "x86".to_string(),
        IMAGE_FILE_MACHINE_ARMNT => "arm".to_string(),
        other => format!("0x{:04x}", other.0),
    }
}

/// Relative to FOLDERID_ProgramFiles; NSIS's fixed `$INSTDIR` (design m5b D.4 step 1).
pub const INSTALL_SUBDIR: &str = r"SHIN DATA CENTER\MKLM";

/// `SHGetKnownFolderPath(FOLDERID_ProgramFiles)` + `INSTALL_SUBDIR`.
///
/// For a 64-bit process (x64 or ARM64, the only builds) this is `%ProgramFiles%` of the machine,
/// the folder NSIS's `$PROGRAMFILES64` names; never taken from the environment.
pub fn fixed_install_dir() -> Result<std::path::PathBuf, Error> {
    // SAFETY: FOLDERID_ProgramFiles is a static GUID; no token. The returned string is freed below.
    let text = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None) }
        .map_err(|error| win32("SHGetKnownFolderPath", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let program_files = std::path::PathBuf::from(OsString::from_wide(unsafe { text.as_wide() }));
    // SAFETY: allocated by SHGetKnownFolderPath with CoTaskMemAlloc; freed once, after the copy.
    unsafe { CoTaskMemFree(Some(text.0.cast_const().cast())) };
    if !program_files.is_absolute() {
        return Err(Error::Insecure {
            path: program_files.display().to_string(),
            reason: "%ProgramFiles% is not an absolute path".to_string(),
        });
    }
    Ok(program_files.join(INSTALL_SUBDIR))
}

/// The native machine of this PC (design m5b C.7: shown by the GUI only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeMachine {
    X64,
    Arm64,
    Other(u16),
}

/// `IsWow64Process2(GetCurrentProcess())`'s native machine.
pub fn native_machine() -> Result<NativeMachine, Error> {
    let mut process = IMAGE_FILE_MACHINE_UNKNOWN;
    let mut native = IMAGE_FILE_MACHINE_UNKNOWN;
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing; both out pointers
    // are valid for the call.
    unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) }
        .map_err(|error| win32("IsWow64Process2", &error))?;
    Ok(match native {
        IMAGE_FILE_MACHINE_AMD64 => NativeMachine::X64,
        IMAGE_FILE_MACHINE_ARM64 => NativeMachine::Arm64,
        other => NativeMachine::Other(other.0),
    })
}

/// Architecture this binary was built for.
fn build_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_names() {
        assert_eq!(machine_name(IMAGE_FILE_MACHINE_AMD64), "x64");
        assert_eq!(machine_name(IMAGE_FILE_MACHINE_ARM64), "arm64");
        assert_eq!(machine_name(IMAGE_FILE_MACHINE(0x1234)), "0x1234");
    }

    #[test]
    fn the_install_folder_is_under_program_files() {
        let dir = fixed_install_dir().expect("install folder");
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.ends_with(r"SHIN DATA CENTER\MKLM"), "{}", dir.display());
        let program_files = dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("parent");
        assert!(program_files.is_dir(), "{}", program_files.display());
        // Not the 32-bit folder, whatever the process.
        assert!(
            !program_files.to_string_lossy().contains("(x86)"),
            "{}",
            program_files.display()
        );
    }

    #[test]
    fn the_native_machine_is_known() {
        let native = native_machine().expect("IsWow64Process2");
        // Design m5b I.6: an x64 build under emulation on ARM64 reports ARM64.
        match std::env::consts::ARCH {
            "aarch64" => assert_eq!(native, NativeMachine::Arm64),
            _ => assert!(matches!(native, NativeMachine::X64 | NativeMachine::Arm64)),
        }
    }
}
