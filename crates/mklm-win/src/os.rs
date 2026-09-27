//! Operating system facts: build number, UBR, native architecture and Remote Desktop.

use mklm_core::OsInfo;
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
    IMAGE_FILE_MACHINE_ARMNT, IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_UNKNOWN, OSVERSIONINFOW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
use windows_registry::LOCAL_MACHINE;

use crate::error::{Error, ReadIssue, ReadIssueKind, win32_code};
use crate::reg::{open_read, read_dword};

/// Key holding `UBR`, relative to `HKEY_LOCAL_MACHINE`.
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Reads the OS facts. Only a failing `RtlGetVersion` is an error; a missing UBR is `None` and an
/// unknown architecture falls back to the one this binary was built for, with an issue.
pub fn read_os_info(issues: &mut Vec<ReadIssue>) -> Result<OsInfo, Error> {
    Ok(OsInfo {
        build: build_number()?,
        ubr: ubr(issues),
        native_arch: native_arch(issues),
        // SAFETY: GetSystemMetrics has no preconditions.
        remote_session: unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0,
    })
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
}
